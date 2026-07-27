//! rege — orquestrador multi-agente de IAs (Rust).
#![allow(dead_code)] // WIP: modules land incrementally

mod agent;
mod buddy;
mod command;
mod config;
mod driver;
mod engine;
mod mcp;
mod playbook;
mod rtk;
mod scan;
mod session;
mod sessions;
mod stream;
mod theme;
mod tmux;
mod transcript;
mod tui;
mod worktree;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use session::Session;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Official upstream used by `rege update` when no `--git` is given.
const REGE_GIT_URL: &str = "https://github.com/Rafatakitani/rege.git";

/// Build profile used only by `update` (see `[profile.fastinstall]` in
/// `Cargo.toml`): it optimizes build time, not the binary.
const FAST_PROFILE: &str = "fastinstall";

/// `0.2.0 (b178154)` — the commit is what actually tells you whether an update
/// landed, since the semver only moves when someone bumps it by hand.
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("REGE_GIT_HASH"), ")");

#[derive(Parser)]
#[command(name = "rege", version = VERSION, about = "Orquestrador multi-agente de IAs")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Roda o mestre headless numa tarefa e imprime (tipo `codex exec`).
    Exec {
        /// A tarefa
        task: Vec<String>,
    },
    /// Checa os bots do roster.
    Doctor,
    /// Mostra a config efetiva.
    Config,
    /// Roda o servidor MCP (JSON-RPC 2.0 newline-delimited sobre stdio).
    McpServe {
        /// Repo alvo pros agentes/worktrees.
        #[arg(long)]
        repo: PathBuf,
    },
    /// Abre o claude INTERATIVO ja como orquestrador Rege (playbook + MCP + yolo).
    Claude,
    /// Atualiza o rege pra última versão (cargo install --git ... --force).
    Update {
        /// URL do repositório (default: upstream oficial).
        #[arg(long, default_value = REGE_GIT_URL)]
        git: String,
        /// Branch, tag ou rev específico (default: branch padrão do repo).
        #[arg(long)]
        branch: Option<String>,
        /// Mostra o output cru do cargo (compilação linha a linha).
        #[arg(long, short)]
        verbose: bool,
        /// Recompila mesmo que o remoto esteja no commit já instalado.
        #[arg(long, short)]
        force: bool,
    },
    /// Escaneia o diretório atual e escreve um AGENTS.md descrevendo ele.
    Scan {
        /// Sobrescreve um AGENTS.md existente.
        #[arg(long)]
        force: bool,
    },
    /// Renderiza um frame da TUI como texto (headless, sem tty) pra inspeção/debug.
    Render {
        /// Semeia estado de exemplo (chat + agentes).
        #[arg(long)]
        demo: bool,
        /// Largura em colunas.
        #[arg(long, default_value_t = 100)]
        cols: u16,
        /// Altura em linhas.
        #[arg(long, default_value_t = 32)]
        rows: u16,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = dirs_home();
    let cwd = std::env::current_dir()?;
    let project = if is_git_repo(&cwd) { Some(cwd.as_path()) } else { None };
    let cfg = Config::load(project, &home)?;
    rtk::configure(&cfg.rtk);

    match cli.cmd {
        Some(Cmd::Exec { task }) => exec(&cfg, &task.join(" ")),
        Some(Cmd::Doctor) => doctor(&cfg),
        Some(Cmd::Config) => {
            print!("{}", serde_yaml::to_string(&cfg)?);
            Ok(())
        }
        Some(Cmd::McpServe { repo }) => mcp_serve(&home, &repo),
        Some(Cmd::Claude) => claude_orchestrator(&cfg),
        Some(Cmd::Update { git, branch, verbose, force }) => update(&git, branch.as_deref(), verbose, force, &home),
        Some(Cmd::Scan { force }) => scan_dir(&cwd, &cfg, &home, force),
        Some(Cmd::Render { demo, cols, rows }) => {
            let repo = cwd.to_string_lossy().to_string();
            println!("{}", tui::render_frame(&cfg, &repo, cols, rows, demo));
            Ok(())
        }
        None => {
            let repo = cwd.to_string_lossy().to_string();
            tui::run(&cfg, &repo)
        }
    }
}

/// Headless run: seed the master with the playbook + task, stream to stdout.
fn exec(cfg: &Config, task: &str) -> Result<()> {
    if task.trim().is_empty() {
        eprintln!("uso: rege exec \"<tarefa>\"");
        std::process::exit(2);
    }
    // exec = headless ORCHESTRATOR (like `codex exec`, but the master commands
    // other agents): the master runs with the playbook + our MCP server, so it
    // can spawn/wait/review workers and open a PR.
    let repo = std::env::current_dir()?;
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "rege".into());
    let mcp = serde_json::json!({
        "mcpServers": { "rege": {
            "command": exe,
            "args": ["mcp-serve", "--repo", repo.to_string_lossy()]
        }}
    })
    .to_string();
    let seed = format!("{}\n\nTarefa: {}", playbook::prompt(cfg), task);

    if cfg.master.cli != "claude" {
        eprintln!("exec orquestrador so suporta master=claude por ora (atual: {})", cfg.master.cli);
        std::process::exit(2);
    }
    let mut a: Vec<String> = vec![
        "claude".into(), "-p".into(), seed,
        "--mcp-config".into(), mcp,
        "--dangerously-skip-permissions".into(),
    ];
    if let Some(m) = &cfg.master.model {
        a.push("--model".into());
        a.push(m.clone());
    }
    let bin = a.remove(0);
    let status = Command::new(bin).args(&a).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Launch claude INTERACTIVE, pre-wired as the Rege orchestrator: playbook
/// system prompt + MCP server + yolo. Same as `exec` but interactive (no -p),
/// so you chat with the master directly (`rege claude` ~ `claude --rege`).
fn claude_orchestrator(cfg: &Config) -> Result<()> {
    if cfg.master.cli != "claude" {
        eprintln!("`rege claude` so suporta master=claude (atual: {})", cfg.master.cli);
        std::process::exit(2);
    }
    let repo = std::env::current_dir()?;
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "rege".into());
    let mcp = serde_json::json!({
        "mcpServers": { "rege": {
            "command": exe,
            "args": ["mcp-serve", "--repo", repo.to_string_lossy()]
        }}
    })
    .to_string();
    let mut a: Vec<String> = vec![
        "claude".into(),
        "--append-system-prompt".into(), playbook::prompt(cfg),
        "--mcp-config".into(), mcp,
        "--dangerously-skip-permissions".into(),
    ];
    if let Some(m) = &cfg.master.model {
        a.push("--model".into());
        a.push(m.clone());
    }
    let bin = a.remove(0);
    let status = Command::new(bin).args(&a).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Self-update: rebuild+reinstall the `rege` binary from git via cargo. No
/// local checkout needed — cargo clones the repo itself and overwrites the
/// binary in `~/.cargo/bin`.
fn update(git: &str, branch: Option<&str>, verbose: bool, force: bool, home: &Path) -> Result<()> {
    // The fastest build is the one that doesn't run. A ~1s query answers
    // whether there is a new commit at all; without it, `rege update` pays a
    // full build to reinstall the very binary already in place — which is what
    // running it out of habit does.
    match remote_head(git, branch) {
        Probe::Sha(head) if !force && already_current(&head, env!("REGE_GIT_HASH")) => {
            println!("rege já está na última ({VERSION}). `--force` reinstala mesmo assim.");
            return Ok(());
        }
        // No network now means no network for cargo's own fetch either: the
        // clone is the first thing it does. Saying so costs a second, where
        // letting cargo find out costs a minute of stalled fetch.
        Probe::Offline => {
            eprintln!("sem rede: não deu pra falar com {git}.");
            eprintln!("o update precisa buscar o repositório — tente de novo quando a conexão voltar.");
            std::process::exit(1);
        }
        _ => {}
    }
    let args = cargo_update_args(git, branch, verbose, Some(FAST_PROFILE));
    let cache = build_cache_dir(home);
    let which = branch.map(|b| format!(" ({b})")).unwrap_or_default();
    // Each profile gets its own artifact subdirectory: the cache root existing
    // says nothing about how fast this build will be. Checking the profile's
    // own directory avoids promising "warm cache" on a one-minute build.
    let first = !warm_cache(&cache, FAST_PROFILE);
    if first {
        println!("atualizando rege{which}… (compilando, ~1min)");
    } else {
        println!("atualizando rege{which}… (compilando, cache quente)");
    }

    // Quiet by default: 90 lines of `Compiling foo v1.2.3` say nothing. The
    // output is captured, not discarded, so a failure still shows why.
    if verbose {
        let ok = run_cargo(&args, &cache, true).map_or_else(|e| cargo_missing(e), |(ok, _)| ok);
        if !ok {
            // Nothing was captured to inspect, so the retry is unconditional:
            // worst case the real error is printed twice.
            let plain = cargo_update_args(git, branch, verbose, None);
            let ok = run_cargo(&plain, &cache, true).map_or_else(|e| cargo_missing(e), |(ok, _)| ok);
            if !ok {
                std::process::exit(1);
            }
        }
        report_installed_version();
        return Ok(());
    }
    let (mut ok, mut err) = run_cargo(&args, &cache, false).unwrap_or_else(|e| cargo_missing(e));
    if !ok && needs_plain_retry(&err) {
        // Old commit, no such profile in its Cargo.toml (or no Cargo.lock).
        // Fall back to release rather than let an optimization break updates.
        let plain = cargo_update_args(git, branch, verbose, None);
        (ok, err) = run_cargo(&plain, &cache, false).unwrap_or_else(|e| cargo_missing(e));
    }
    if !ok {
        eprint!("{}", tail_lines(&err, 20));
        if looks_like_network_failure(&err) {
            eprintln!("o fetch do repositório não passou — sem rede, ou DNS/proxy no caminho.");
        }
        eprintln!("falha ao atualizar. `rege update --verbose` pro output completo.");
        std::process::exit(1);
    }
    report_installed_version();
    Ok(())
}

/// Roda o cargo, devolvendo (sucesso, stderr). Em modo verboso o output vai
/// direto pro terminal e o stderr volta vazio.
fn run_cargo(args: &[String], cache: &Path, verbose: bool) -> std::io::Result<(bool, String)> {
    let mut cmd = Command::new("cargo");
    cmd.args(args).env("CARGO_TARGET_DIR", cache);
    // Cargo's bundled libgit2 does its own name resolution and trips where the
    // system git sails through ("failed to resolve address for github.com" on a
    // working network). Delegating the fetch is what cargo itself suggests in
    // that error, and it reuses credentials and proxies already configured.
    cmd.env("CARGO_NET_GIT_FETCH_WITH_CLI", "true");
    if verbose {
        return cmd.status().map(|s| (s.success(), String::new()));
    }
    cmd.output().map(|o| (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned()))
}

/// What asking the remote told us. `Unknown` covers git missing from PATH, a
/// ref that doesn't exist, an auth prompt — cases where the build should go
/// ahead and let cargo deliver the verdict. `Offline` is separated out because
/// there the build is guaranteed to fail too, only a minute later.
#[derive(Debug, PartialEq)]
enum Probe {
    Sha(String),
    Offline,
    Unknown,
}

/// The commit the remote would hand over, without cloning anything.
fn remote_head(git: &str, branch: Option<&str>) -> Probe {
    let Ok(out) = Command::new("git").args(["ls-remote", git, branch.unwrap_or("HEAD")]).output() else {
        return Probe::Unknown;
    };
    if !out.status.success() {
        return classify_ls_remote_failure(&String::from_utf8_lossy(&out.stderr));
    }
    match parse_ls_remote(&String::from_utf8_lossy(&out.stdout)) {
        Some(sha) => Probe::Sha(sha),
        None => Probe::Unknown,
    }
}

/// A failed `ls-remote` is only worth aborting on when the network is the
/// reason. Anything else (missing ref, credentials) is cargo's call to make.
fn classify_ls_remote_failure(stderr: &str) -> Probe {
    if looks_like_network_failure(stderr) {
        Probe::Offline
    } else {
        Probe::Unknown
    }
}

/// Primeiro SHA de uma saída de `git ls-remote` (`<sha>\t<ref>` por linha).
fn parse_ls_remote(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|l| l.split_whitespace().next()).map(str::to_string).filter(|s| s.len() >= 7)
}

/// O binário rodando já é esse commit? `local` é o hash curto carimbado no
/// build; `sem-git` (build de tarball) não prova nada e nunca dispensa o build.
fn already_current(remote: &str, local: &str) -> bool {
    local != "sem-git" && local.len() >= 7 && remote.starts_with(local)
}

/// Falha de rede ou de resolução de nome, não de compilação — merece uma dica
/// diferente do "olha o log completo".
fn looks_like_network_failure(stderr: &str) -> bool {
    [
        "network failure",
        "failed to resolve address",
        "failed to fetch into",
        "Could not resolve host",
        "Name or service not known",
        "Could not read from remote repository",
        "Connection timed out",
        "Network is unreachable",
    ]
        .iter()
        .any(|m| stderr.contains(m))
}

/// Distingue "esse commit é velho demais pros flags rápidos" de uma falha de
/// compilação de verdade — só o primeiro caso merece uma segunda tentativa.
/// Perfil ausente: "profile `fastinstall` is not defined". Lock ausente ou
/// desatualizado: o cargo cita o próprio `--locked` na mensagem.
fn needs_plain_retry(stderr: &str) -> bool {
    (stderr.contains(FAST_PROFILE) && stderr.contains("is not defined")) || stderr.contains("--locked")
}

/// Prints what actually got installed. "run `rege --version` to check" put the
/// work on the user for information the update already has — and the commit is
/// the part that proves the new build landed.
fn report_installed_version() {
    let installed = Command::new("rege")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    match installed {
        Some(v) => println!("✓ atualizado: {v} (era {VERSION})"),
        None => println!("✓ rege atualizado. `rege --version` pra conferir."),
    }
}

/// `cargo install --git` builds into a throwaway temp dir, so every update
/// recompiles all ~95 dependencies from scratch. Pointing it at a persistent
/// target dir means only the crates that actually changed get rebuilt. It's a
/// cache — safe to delete, costs disk.
fn build_cache_dir(home: &Path) -> PathBuf {
    home.join(".cache/rege/build")
}

/// Há artefatos reaproveitáveis pra este perfil? O cargo guarda cada perfil num
/// subdiretório do target dir, então é ele que responde — e vazio conta como
/// frio, que é o estado logo depois de criar o diretório.
fn warm_cache(cache: &Path, profile: &str) -> bool {
    std::fs::read_dir(cache.join(profile)).is_ok_and(|mut d| d.next().is_some())
}

fn cargo_missing(e: std::io::Error) -> ! {
    eprintln!("falha ao rodar cargo (instalado? no PATH?): {e}");
    std::process::exit(1);
}

/// Last `n` lines — cargo puts the actual error at the end, after the wall of
/// `Compiling` noise.
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    let mut out = lines[start..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The `cargo install` argv for a self-update. Split out so the flag wiring is
/// unit-testable without shelling out.
fn cargo_update_args(git: &str, branch: Option<&str>, verbose: bool, profile: Option<&str>) -> Vec<String> {
    let mut a = vec!["install".to_string(), "--git".to_string(), git.to_string(), "--force".to_string()];
    if let Some(b) = branch {
        a.push("--branch".to_string());
        a.push(b.to_string());
    }
    if let Some(p) = profile {
        // `--locked` travels with the fast profile: without it cargo re-resolves
        // all 94 dependencies and refreshes the crates.io index on every update
        // — expensive, and it installs versions nobody tested. Both need a
        // recent commit (profile + Cargo.lock in the repo), and both drop
        // together in the fallback when it isn't.
        a.push("--profile".to_string());
        a.push(p.to_string());
        a.push("--locked".to_string());
    }
    if !verbose {
        a.push("--quiet".to_string());
    }
    a
}

/// One-shot context scan of `dir`. Also records the directory as answered, so
/// the TUI doesn't turn around and offer what was just done by hand.
fn scan_dir(dir: &Path, cfg: &Config, home: &Path, force: bool) -> Result<()> {
    println!("escaneando {}… (uma chamada ao mestre)", dir.display());
    let path = scan::run(dir, cfg, home, force)?;
    let _ = scan::record(&scan::state_path(home), dir, "yes");
    println!("✓ escrito: {}", path.display());
    Ok(())
}

/// Instantiate a Session/Engine for `repo` and serve MCP over stdin/stdout.
fn mcp_serve(home: &Path, repo: &Path) -> Result<()> {
    let cfg = Config::load(Some(repo), home)?;
    rtk::configure(&cfg.rtk);
    let session = Session::new(repo, &cfg);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = mcp::Server::new(session, stdin.lock(), stdout.lock());
    server.run()
}

fn doctor(cfg: &Config) -> Result<()> {
    if let Some(m) = &cfg.master.model {
        println!("mestre: {} ({})", cfg.master.cli, m);
    } else {
        println!("mestre: {}", cfg.master.cli);
    }
    println!("health check:");
    for cli in cfg.distinct_clis() {
        let ok = probe_ok(&cli);
        println!("  {} {}", if ok { "✓" } else { "✗" }, cli);
    }
    Ok(())
}

fn probe_ok(cli: &str) -> bool {
    let Ok(mut a) = command::probe(cli) else { return false };
    let bin = a.remove(0);
    Command::new(bin)
        .args(&a)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn is_git_repo(dir: &std::path::Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-dir"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_remote_gives_the_sha_and_ignores_the_ref() {
        assert_eq!(parse_ls_remote("5bf8410c0d\trefs/heads/main\n"), Some("5bf8410c0d".into()));
        // Várias linhas (branch + tag anotada): a primeira basta.
        assert_eq!(parse_ls_remote("abc1234567\tHEAD\nzzz\trefs/tags/v1\n"), Some("abc1234567".into()));
        assert_eq!(parse_ls_remote(""), None);
        // Junk too short to be a SHA doesn't get to pass for one.
        assert_eq!(parse_ls_remote("nope\tref"), None);
    }

    #[test]
    fn offline_is_told_apart_from_a_ref_that_does_not_exist() {
        // Aborting is only right when cargo's own fetch is doomed too.
        assert_eq!(
            classify_ls_remote_failure("fatal: unable to access 'https://x/': Could not resolve host: github.com"),
            Probe::Offline
        );
        assert_eq!(
            classify_ls_remote_failure("ssh: Could not resolve hostname github.com: Name or service not known"),
            Probe::Offline
        );
        // Auth or a missing ref: let cargo decide, same as before.
        assert_eq!(classify_ls_remote_failure("fatal: Authentication failed"), Probe::Unknown);
        assert_eq!(classify_ls_remote_failure(""), Probe::Unknown);
    }

    #[test]
    fn already_current_only_when_the_stamp_proves_it() {
        assert!(already_current("5bf8410c0dfeed", "5bf8410"), "prefixo do curto bate");
        assert!(!already_current("a47f4f1c0dfeed", "5bf8410"), "commit novo no remoto");
        // A build with no git (tarball, vendor) can't know where it stands.
        assert!(!already_current("5bf8410c0dfeed", "sem-git"));
        assert!(!already_current("5bf8410c0dfeed", ""));
    }

    #[test]
    fn network_failure_gets_its_own_hint() {
        let real = "error: failed to fetch into: /home/u/.cargo/git/db/rege-e7e\n  network failure seems to have happened";
        assert!(looks_like_network_failure(real));
        assert!(looks_like_network_failure("failed to resolve address for github.com"));
        // A real compile error must not turn into "no network".
        assert!(!looks_like_network_failure("error[E0308]: mismatched types"));
    }

    #[test]
    fn cargo_update_args_defaults_to_forced_quiet_git_install() {
        assert_eq!(
            cargo_update_args(REGE_GIT_URL, None, false, None),
            vec!["install", "--git", REGE_GIT_URL, "--force", "--quiet"]
        );
    }

    #[test]
    fn cargo_update_args_appends_branch() {
        let a = cargo_update_args("https://x/y.git", Some("dev"), false, None);
        assert_eq!(
            a,
            vec!["install", "--git", "https://x/y.git", "--force", "--branch", "dev", "--quiet"]
        );
    }

    #[test]
    fn cargo_update_args_verbose_drops_quiet() {
        let a = cargo_update_args("https://x/y.git", None, true, None);
        assert_eq!(a, vec!["install", "--git", "https://x/y.git", "--force"]);
    }

    #[test]
    fn cargo_update_args_passes_the_fast_profile_and_locks() {
        let a = cargo_update_args("https://x/y.git", None, false, Some(FAST_PROFILE));
        assert_eq!(
            a,
            vec![
                "install",
                "--git",
                "https://x/y.git",
                "--force",
                "--profile",
                "fastinstall",
                "--locked",
                "--quiet"
            ]
        );
    }

    #[test]
    fn plain_retry_only_for_the_old_commit_case() {
        assert!(needs_plain_retry("error: profile `fastinstall` is not defined"));
        assert!(needs_plain_retry("error: the lock file needs to be updated but --locked was passed"));
        // A real compile failure must not turn into a silent fallback.
        assert!(!needs_plain_retry("error[E0308]: mismatched types"));
        assert!(!needs_plain_retry("error: profile `bench` is not defined"));
    }

    #[test]
    fn warm_cache_looks_at_the_profile_not_the_root() {
        let d = std::env::temp_dir().join(format!("rege-warm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(!warm_cache(&d, FAST_PROFILE), "sem o subdiretório do perfil é frio");
        std::fs::create_dir_all(d.join(FAST_PROFILE)).unwrap();
        assert!(!warm_cache(&d, FAST_PROFILE), "subdiretório vazio ainda é frio");
        std::fs::write(d.join(FAST_PROFILE).join("rege"), "x").unwrap();
        assert!(warm_cache(&d, FAST_PROFILE));
        // Another profile's cache doesn't count.
        assert!(!warm_cache(&d, "release"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `--locked` exige o `Cargo.lock` versionado; se ele sair do repo todo
    /// update passa a pagar dois builds.
    #[test]
    fn cargo_lock_is_committed() {
        let lock = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock");
        assert!(std::path::Path::new(lock).exists(), "Cargo.lock sumiu");
    }

    /// O perfil que o `update` pede tem que existir aqui, senão todo update
    /// paga o fallback (dois builds) em vez de um.
    #[test]
    fn the_fast_profile_exists_in_cargo_toml() {
        let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
        assert!(toml.contains(&format!("[profile.{FAST_PROFILE}]")), "perfil sumiu do Cargo.toml");
    }

    #[test]
    fn tail_lines_keeps_the_end_where_cargo_puts_the_error() {
        let text = "Compiling a\nCompiling b\nerror: boom\n";
        assert_eq!(tail_lines(text, 2), "Compiling b\nerror: boom\n");
        // Shorter than the window: everything, unchanged.
        assert_eq!(tail_lines("só isso\n", 20), "só isso\n");
        assert_eq!(tail_lines("", 20), "");
    }
}
