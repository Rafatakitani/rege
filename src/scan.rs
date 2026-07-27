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
    s.push_str("Write the contents of an AGENTS.md file for the directory below.\n");
    s.push_str("It is a short document guiding AI agents that will work here.\n\n");
    if f.is_home {
        s.push_str(
            "WARNING: this directory is the user's HOME, not a project. \
             Describe it as a personal workspace — what lives here and how to get around. \
             Do not pretend it is a monorepo and do not describe software architecture.\n\n",
        );
    }
    s.push_str(&format!("directory: {}\n", f.dir.display()));
    match (&f.branch, &f.remote) {
        (Some(b), Some(r)) => s.push_str(&format!("git: branch {b}, remote {r}\n")),
        (Some(b), None) => s.push_str(&format!("git: branch {b}, no remote\n")),
        _ => s.push_str("git: not a repository\n"),
    }
    if !f.langs.is_empty() {
        let langs: Vec<String> = f.langs.iter().map(|(e, n)| format!(".{e} ({n})")).collect();
        s.push_str(&format!("most common extensions: {}\n", langs.join(", ")));
    }
    if !f.markers.is_empty() {
        s.push_str(&format!("manifests: {}\n", f.markers.join(", ")));
    }
    if !f.commands.is_empty() {
        s.push_str(&format!("likely commands: {}\n", f.commands.join(" · ")));
    }
    if f.truncated {
        s.push_str("(walk truncated at the file ceiling — the listing is partial)\n");
    }
    if !f.tree.is_empty() {
        s.push_str(&format!("\nstructure (up to 2 levels):\n{}\n", f.tree.join("\n")));
    }
    if let Some(r) = &f.readme {
        s.push_str(&format!("\nREADME (beginning):\n{r}\n"));
    }
    s.push_str(
        "\nIf an AGENTS.md already exists in this directory, ignore it: you are writing \
         the new version from scratch. Do not ask anything, do not offer options, do not \
         comment on the task — your entire answer becomes the file, literally.\n",
    );
    s.push_str(
        "\nYou have no tools in this session and do not need any: everything that belongs \
         in the file is already in the data above. Do not try to write the file yourself \
         and do not add any warning about not having inspected the repo.\n",
    );
    s.push_str(
        "\nAnswer ONLY with the file's markdown, with no code fences around it and no \
         comments of your own. Suggested sections: what it is, how to run/test it, \
         structure, conventions. Be specific and short; do not invent anything that \
         is not in the data above.\n",
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
        bail!("{} already exists — use --force to overwrite", target.display());
    }
    let facts = collect(dir, home);
    let mut argv = command::argv(&cfg.master.cli, &prompt(&facts), cfg.master.model.as_deref(), false)?;
    argv.extend(command::text_only_flags(&cfg.master.cli));
    let out = Command::new(&argv[0]).args(&argv[1..]).current_dir(dir).output()?;
    if !out.status.success() {
        bail!("{} failed: {}", cfg.master.cli, String::from_utf8_lossy(&out.stderr).trim());
    }
    let body = strip_fences(String::from_utf8_lossy(&out.stdout).trim());
    if body.is_empty() {
        bail!("the master answered with nothing");
    }
    if !looks_like_a_document(&body) {
        bail!(
            "the master answered with conversation, not a document — nothing was written. \
             answer:\n{body}"
        );
    }
    std::fs::write(&target, format!("{body}\n"))?;
    Ok(target)
}

/// The master's stdout becomes the file byte for byte, so a chatty answer
/// ("already exists, would you like me to…") would silently replace a hand-written AGENTS.md.
/// A real document opens with a heading and isn't three lines long.
fn looks_like_a_document(body: &str) -> bool {
    let has_heading = body.lines().next().is_some_and(|l| l.trim_start().starts_with('#'));
    has_heading && body.lines().filter(|l| !l.trim().is_empty()).count() >= 3
}

/// Models like wrapping a whole file in ```markdown despite being told not to,
/// and sometimes bracket it with chatter ("couldn't inspect the repo, but…").
/// When a fenced block is present, that block *is* the file — anything outside
/// it is the model talking to us, not content.
fn strip_fences(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(open) = lines.iter().position(|l| l.trim_start().starts_with("```")) else {
        return text.to_string();
    };
    let Some(close) = lines.iter().rposition(|l| l.trim() == "```") else { return text.to_string() };
    // A real document's own code blocks must survive. Treat the outer fence as
    // a wrapper only when nothing before it looks like content (no heading) and
    // the closing fence really ends the answer.
    let preamble_is_chatter = !lines[..open].iter().any(|l| l.trim_start().starts_with('#'));
    let closes_the_answer = lines[close + 1..].iter().all(|l| l.trim().is_empty());
    if close > open && preamble_is_chatter && closes_the_answer {
        return lines[open + 1..close].join("\n").trim().to_string();
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
        assert!(prompt(&f).contains("user's HOME"), "the prompt has to warn the model");
    }

    #[test]
    fn commands_dedupe_across_manifests() {
        let markers = vec!["pyproject.toml".to_string(), "requirements.txt".to_string()];
        assert_eq!(commands_for(&markers), vec!["pytest"], "pytest must not appear twice");
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
        assert!(p.contains("not a repository"));
    }

    #[test]
    fn should_offer_only_when_untouched_and_unasked() {
        let d = tmp("offer");
        let mut state = Scanned::default();
        assert!(should_offer(&d, &state), "pasta virgem: pergunta");

        state.dirs.insert(d.to_string_lossy().to_string(), "no".into());
        assert!(!should_offer(&d, &state), "already declined: never again");

        let d2 = tmp("offer-existing");
        fs::write(d2.join(CONTEXT_FILE), "# já tem\n").unwrap();
        assert!(!should_offer(&d2, &Scanned::default()), "AGENTS.md is there: do not ask");
    }

    #[test]
    fn state_roundtrips_through_yaml() {
        let d = tmp("state");
        let p = d.join("config/rege/scanned.yml");
        assert_eq!(load_state(&p), Scanned::default(), "missing file = empty state");

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
        let chat = "AGENTS.md already exists, with very specific content.\n\nWould you like me to:\n1. keep it,\n2. update it?";
        assert!(!looks_like_a_document(chat));
        assert!(!looks_like_a_document("# Projeto\n\nfaz X."), "two lines is an answer, not a file");
        assert!(looks_like_a_document("# Projeto\n\nfaz X.\n\n## Testes\n\n`cargo test`"));
    }

    #[test]
    fn prompt_tells_the_master_not_to_chat() {
        let d = tmp("no-chat");
        fs::write(d.join(CONTEXT_FILE), "# já tem\n").unwrap();
        let p = prompt(&collect(&d, Path::new("/outro")));
        assert!(p.contains("ignore it"), "the model has to know to ignore the current file");
        assert!(p.contains("Do not ask anything"));
    }

    #[test]
    fn strip_fences_unwraps_a_fenced_answer() {
        assert_eq!(strip_fences("```markdown\n# Oi\n```"), "# Oi");
        assert_eq!(strip_fences("# Oi\n\ntexto"), "# Oi\n\ntexto");
        // Fence only at the start isn't a wrapper — leave it alone.
        assert_eq!(strip_fences("```sh\nls\n\ntexto"), "```sh\nls\n\ntexto");
        // Chatter before the wrapper: the block is the file, the chatter isn't.
        assert_eq!(strip_fences("Sem tool de leitura aqui.\n\n```markdown\n# Oi\n\ntexto\n```"), "# Oi\n\ntexto");
        // A document's OWN code blocks must survive untouched.
        let doc = "# Projeto\n\n## Testes\n\n```sh\ncargo test\n```";
        assert_eq!(strip_fences(doc), doc);
        let ends_fenced = "# Projeto\n\nrode:\n\n```sh\nls\n```\n";
        assert_eq!(strip_fences(ends_fenced), ends_fenced);
    }
}
