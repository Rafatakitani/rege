//! First-run context scan: looks at the directory rege was opened in and
//! writes an `AGENTS.md` describing it, the way `/init` does for Claude Code.
//!
//! Split in two halves on purpose. **Collection** is deterministic and cheap —
//! plain filesystem and git facts, no model involved, hard-capped so opening
//! rege in `~/` can't turn into a full-disk crawl. **Writing** hands that
//! summary to the master in one shot; the model interprets a digest instead of
//! walking the tree itself, which keeps the cost near zero and works outside a
//! git repo (a home directory is not a worktree).

use crate::command;
use crate::config::Config;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The file the scan produces. `AGENTS.md` is the convention claude, codex and
/// opencode already read on their own, so the context reaches every worker
/// without rege injecting anything into their prompts.
pub const CONTEXT_FILE: &str = "AGENTS.md";

/// Directories never worth describing — build output and dependency dumps say
/// nothing about what a project *is*.
const IGNORED: &[&str] = &[
    ".git", "node_modules", "target", "vendor", "dist", "build", ".venv", "venv", "__pycache__",
    ".next", ".cache", "coverage",
];

/// Ceiling on files visited. A home directory has no natural bottom.
const MAX_FILES: usize = 4000;
const MAX_DEPTH: usize = 2;
const TOP_LANGS: usize = 8;
const README_LINES: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub dir: PathBuf,
    /// The scan target is the user's home — describe a workspace, not a project.
    pub is_home: bool,
    pub branch: Option<String>,
    pub remote: Option<String>,
    /// Extension → file count, biggest first.
    pub langs: Vec<(String, usize)>,
    /// Build/dependency manifests found at the top level.
    pub markers: Vec<String>,
    /// Build and test commands inferred from those manifests.
    pub commands: Vec<String>,
    pub readme: Option<String>,
    /// Entries up to `MAX_DEPTH`, relative to `dir`.
    pub tree: Vec<String>,
    pub truncated: bool,
}

pub fn collect(dir: &Path, home: &Path) -> Facts {
    let mut langs: BTreeMap<String, usize> = BTreeMap::new();
    let mut tree = Vec::new();
    let mut seen = 0usize;
    walk(dir, dir, 0, &mut seen, &mut langs, &mut tree);

    let mut langs: Vec<(String, usize)> = langs.into_iter().collect();
    langs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    langs.truncate(TOP_LANGS);
    tree.sort();

    let markers = markers_in(dir);
    Facts {
        dir: dir.to_path_buf(),
        is_home: same_path(dir, home),
        branch: git_capture(dir, &["rev-parse", "--abbrev-ref", "HEAD"]),
        remote: git_capture(dir, &["remote", "get-url", "origin"]),
        langs,
        commands: commands_for(&markers),
        markers,
        readme: read_head(dir),
        tree,
        truncated: seen >= MAX_FILES,
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    seen: &mut usize,
    langs: &mut BTreeMap<String, usize>,
    tree: &mut Vec<String>,
) {
    if depth > MAX_DEPTH || *seen >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *seen >= MAX_FILES {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if IGNORED.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        let is_dir = path.is_dir();
        if let Ok(rel) = path.strip_prefix(root) {
            let shown = rel.to_string_lossy().to_string();
            tree.push(if is_dir { format!("{shown}/") } else { shown });
        }
        if is_dir {
            walk(root, &path, depth + 1, seen, langs, tree);
            continue;
        }
        *seen += 1;
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            *langs.entry(ext.to_string()).or_insert(0) += 1;
        }
    }
}

const MARKERS: &[(&str, &[&str])] = &[
    ("Cargo.toml", &["cargo build", "cargo test"]),
    ("package.json", &["npm install", "npm test"]),
    ("go.mod", &["go build ./...", "go test ./..."]),
    ("pyproject.toml", &["pytest"]),
    ("requirements.txt", &["pytest"]),
    ("Gemfile", &["bundle install", "bundle exec rspec"]),
    ("Makefile", &["make"]),
    ("docker-compose.yml", &["docker compose up"]),
];

fn markers_in(dir: &Path) -> Vec<String> {
    MARKERS.iter().map(|(m, _)| *m).filter(|m| dir.join(m).exists()).map(String::from).collect()
}

/// Commands implied by the manifests present, deduped and in manifest order.
fn commands_for(markers: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (marker, cmds) in MARKERS {
        if !markers.iter().any(|m| m == marker) {
            continue;
        }
        for c in *cmds {
            if !out.iter().any(|o| o == c) {
                out.push((*c).to_string());
            }
        }
    }
    out
}

fn read_head(dir: &Path) -> Option<String> {
    for name in ["README.md", "README", "readme.md"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            let head: Vec<&str> = text.lines().take(README_LINES).collect();
            return Some(head.join("\n"));
        }
    }
    None
}

fn git_capture(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The one-shot request handed to the master. Facts go in as a digest so the
/// model spends its budget on judgement, not on `ls`.
pub fn prompt(f: &Facts) -> String {
    let mut s = String::new();
    s.push_str("Escreva o conteúdo de um arquivo AGENTS.md para o diretório abaixo.\n");
    s.push_str("É um documento curto que orienta agentes de IA que vão trabalhar aqui.\n\n");
    if f.is_home {
        s.push_str(
            "ATENÇÃO: este diretório é a HOME do usuário, não um projeto. \
             Descreva-o como espaço de trabalho pessoal — o que mora aqui e como se orientar. \
             Não invente que é um monorepo nem descreva arquitetura de software.\n\n",
        );
    }
    s.push_str(&format!("diretório: {}\n", f.dir.display()));
    match (&f.branch, &f.remote) {
        (Some(b), Some(r)) => s.push_str(&format!("git: branch {b}, remote {r}\n")),
        (Some(b), None) => s.push_str(&format!("git: branch {b}, sem remote\n")),
        _ => s.push_str("git: não é um repositório\n"),
    }
    if !f.langs.is_empty() {
        let langs: Vec<String> = f.langs.iter().map(|(e, n)| format!(".{e} ({n})")).collect();
        s.push_str(&format!("extensões mais comuns: {}\n", langs.join(", ")));
    }
    if !f.markers.is_empty() {
        s.push_str(&format!("manifestos: {}\n", f.markers.join(", ")));
    }
    if !f.commands.is_empty() {
        s.push_str(&format!("comandos prováveis: {}\n", f.commands.join(" · ")));
    }
    if f.truncated {
        s.push_str("(varredura truncada no teto de arquivos — a listagem é parcial)\n");
    }
    if !f.tree.is_empty() {
        s.push_str(&format!("\nestrutura (até 2 níveis):\n{}\n", f.tree.join("\n")));
    }
    if let Some(r) = &f.readme {
        s.push_str(&format!("\nREADME (início):\n{r}\n"));
    }
    s.push_str(
        "\nSe já existir um AGENTS.md neste diretório, ignore-o: você está escrevendo \
         a versão nova do zero. Não pergunte nada, não ofereça opções, não comente \
         a tarefa — a sua resposta inteira vai virar o arquivo, literalmente.\n",
    );
    s.push_str(
        "\nResponda APENAS com o markdown do arquivo, sem cercas de código em volta \
         e sem comentários seus. Seções sugeridas: o que é, como rodar/testar, \
         estrutura, convenções. Seja específico e curto; não invente o que não \
         estiver nos dados acima.\n",
    );
    s
}

/// Directories already offered a scan, so a "no" is remembered. Lives in
/// `~/.config/rege/`, never in the user's project — declining shouldn't leave
/// a file behind in someone else's git status.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Scanned {
    #[serde(default)]
    pub dirs: BTreeMap<String, String>,
}

pub fn state_path(home: &Path) -> PathBuf {
    home.join(".config/rege/scanned.yml")
}

pub fn load_state(path: &Path) -> Scanned {
    std::fs::read_to_string(path).ok().and_then(|t| serde_yaml::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_state(path: &Path, state: &Scanned) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_yaml::to_string(state)?)?;
    Ok(())
}

pub fn record(path: &Path, dir: &Path, answer: &str) -> Result<()> {
    let mut state = load_state(path);
    state.dirs.insert(dir.to_string_lossy().to_string(), answer.to_string());
    save_state(path, &state)
}

/// Ask only where there's nothing to lose: no context file yet, and the user
/// was never asked about this directory before.
pub fn should_offer(dir: &Path, state: &Scanned) -> bool {
    !dir.join(CONTEXT_FILE).exists() && !state.dirs.contains_key(&dir.to_string_lossy().to_string())
}

/// Collects, asks the master, writes the file. Returns the path written.
pub fn run(dir: &Path, cfg: &Config, home: &Path, force: bool) -> Result<PathBuf> {
    let target = dir.join(CONTEXT_FILE);
    if target.exists() && !force {
        bail!("{} já existe — use --force pra sobrescrever", target.display());
    }
    let facts = collect(dir, home);
    let argv = command::argv(&cfg.master.cli, &prompt(&facts), cfg.master.model.as_deref(), false)?;
    let out = Command::new(&argv[0]).args(&argv[1..]).current_dir(dir).output()?;
    if !out.status.success() {
        bail!("{} falhou: {}", cfg.master.cli, String::from_utf8_lossy(&out.stderr).trim());
    }
    let body = strip_fences(String::from_utf8_lossy(&out.stdout).trim());
    if body.is_empty() {
        bail!("o mestre respondeu vazio");
    }
    if !looks_like_a_document(&body) {
        bail!(
            "o mestre respondeu conversando, não com um documento — nada foi escrito. \
             resposta:\n{body}"
        );
    }
    std::fs::write(&target, format!("{body}\n"))?;
    Ok(target)
}

/// The master's stdout becomes the file byte for byte, so a chatty answer
/// ("já existe, quer que eu…") would silently replace a hand-written AGENTS.md.
/// A real document opens with a heading and isn't three lines long.
fn looks_like_a_document(body: &str) -> bool {
    let has_heading = body.lines().next().is_some_and(|l| l.trim_start().starts_with('#'));
    has_heading && body.lines().filter(|l| !l.trim().is_empty()).count() >= 3
}

/// Models like wrapping a whole file in ```markdown despite being told not to.
fn strip_fences(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let opens = lines.first().is_some_and(|l| l.trim_start().starts_with("```"));
    let closes = lines.last().is_some_and(|l| l.trim() == "```");
    if opens && closes && lines.len() >= 2 {
        return lines[1..lines.len() - 1].join("\n").trim().to_string();
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-scan-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn collect_reads_langs_markers_commands_and_tree() {
        let d = tmp("collect");
        fs::write(d.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(d.join("README.md"), "# Projeto\n\nfaz X.\n").unwrap();
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(d.join("src/lib.rs"), "\n").unwrap();
        // Ignored dirs never reach the digest.
        fs::create_dir_all(d.join("target/debug")).unwrap();
        fs::write(d.join("target/debug/junk.rs"), "\n").unwrap();

        let f = collect(&d, Path::new("/nao/eh/home"));
        assert_eq!(f.langs.first().unwrap(), &("rs".to_string(), 2), "só os .rs de src/");
        assert_eq!(f.markers, vec!["Cargo.toml"]);
        assert_eq!(f.commands, vec!["cargo build", "cargo test"]);
        assert!(f.readme.unwrap().contains("faz X"));
        assert!(f.tree.iter().any(|t| t == "src/"));
        assert!(!f.tree.iter().any(|t| t.contains("target")), "target/ fica de fora");
        assert!(!f.is_home);
        assert!(!f.truncated);
    }

    #[test]
    fn collect_flags_the_home_directory() {
        let d = tmp("home");
        let f = collect(&d, &d);
        assert!(f.is_home);
        assert!(prompt(&f).contains("HOME do usuário"), "o prompt precisa avisar o modelo");
    }

    #[test]
    fn commands_dedupe_across_manifests() {
        let markers = vec!["pyproject.toml".to_string(), "requirements.txt".to_string()];
        assert_eq!(commands_for(&markers), vec!["pytest"], "pytest não entra duas vezes");
    }

    #[test]
    fn prompt_carries_the_collected_facts() {
        let d = tmp("prompt");
        fs::write(d.join("go.mod"), "module x\n").unwrap();
        fs::write(d.join("main.go"), "package main\n").unwrap();
        let p = prompt(&collect(&d, Path::new("/outro")));
        assert!(p.contains("go.mod"));
        assert!(p.contains("go test ./..."));
        assert!(p.contains(".go (1)"));
        assert!(p.contains("não é um repositório"));
    }

    #[test]
    fn should_offer_only_when_untouched_and_unasked() {
        let d = tmp("offer");
        let mut state = Scanned::default();
        assert!(should_offer(&d, &state), "pasta virgem: pergunta");

        state.dirs.insert(d.to_string_lossy().to_string(), "no".into());
        assert!(!should_offer(&d, &state), "já recusou: nunca mais");

        let d2 = tmp("offer-existing");
        fs::write(d2.join(CONTEXT_FILE), "# já tem\n").unwrap();
        assert!(!should_offer(&d2, &Scanned::default()), "já tem AGENTS.md: não pergunta");
    }

    #[test]
    fn state_roundtrips_through_yaml() {
        let d = tmp("state");
        let p = d.join("config/rege/scanned.yml");
        assert_eq!(load_state(&p), Scanned::default(), "arquivo ausente = estado vazio");

        record(&p, Path::new("/projeto/x"), "no").unwrap();
        record(&p, Path::new("/projeto/y"), "yes").unwrap();
        let back = load_state(&p);
        assert_eq!(back.dirs.get("/projeto/x").map(String::as_str), Some("no"));
        assert_eq!(back.dirs.get("/projeto/y").map(String::as_str), Some("yes"));
    }

    #[test]
    fn run_refuses_to_clobber_an_existing_file() {
        let d = tmp("clobber");
        fs::write(d.join(CONTEXT_FILE), "escrito à mão\n").unwrap();
        let err = run(&d, &Config::default(), Path::new("/home/x"), false).unwrap_err();
        assert!(err.to_string().contains("--force"));
        assert_eq!(fs::read_to_string(d.join(CONTEXT_FILE)).unwrap(), "escrito à mão\n");
    }

    #[test]
    fn a_chatty_answer_is_not_a_document() {
        // The real regression: `scan --force` replaced a 88-line AGENTS.md with this.
        let chat = "AGENTS.md já existe, com conteúdo bem específico.\n\nQuer que eu:\n1. mantenha,\n2. atualize?";
        assert!(!looks_like_a_document(chat));
        assert!(!looks_like_a_document("# Projeto\n\nfaz X."), "duas linhas é resposta, não arquivo");
        assert!(looks_like_a_document("# Projeto\n\nfaz X.\n\n## Testes\n\n`cargo test`"));
    }

    #[test]
    fn prompt_tells_the_master_not_to_chat() {
        let d = tmp("no-chat");
        fs::write(d.join(CONTEXT_FILE), "# já tem\n").unwrap();
        let p = prompt(&collect(&d, Path::new("/outro")));
        assert!(p.contains("ignore-o"), "o modelo precisa saber pra ignorar o arquivo atual");
        assert!(p.contains("Não pergunte nada"));
    }

    #[test]
    fn strip_fences_unwraps_a_fenced_answer() {
        assert_eq!(strip_fences("```markdown\n# Oi\n```"), "# Oi");
        assert_eq!(strip_fences("# Oi\n\ntexto"), "# Oi\n\ntexto");
        // Fence only at the start isn't a wrapper — leave it alone.
        assert_eq!(strip_fences("```sh\nls\n\ntexto"), "```sh\nls\n\ntexto");
    }
}
