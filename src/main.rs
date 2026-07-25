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
mod session;
mod sessions;
mod stream;
mod theme;
mod tmux;
mod tui;
mod worktree;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use session::Session;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Upstream oficial usado por `rege update` quando nenhum `--git` é passado.
const REGE_GIT_URL: &str = "https://github.com/Rafatakitani/rege.git";

#[derive(Parser)]
#[command(name = "rege", version, about = "Orquestrador multi-agente de IAs")]
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

    match cli.cmd {
        Some(Cmd::Exec { task }) => exec(&cfg, &task.join(" ")),
        Some(Cmd::Doctor) => doctor(&cfg),
        Some(Cmd::Config) => {
            print!("{}", serde_yaml::to_string(&cfg)?);
            Ok(())
        }
        Some(Cmd::McpServe { repo }) => mcp_serve(&home, &repo),
        Some(Cmd::Claude) => claude_orchestrator(&cfg),
        Some(Cmd::Update { git, branch }) => update(&git, branch.as_deref()),
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
fn update(git: &str, branch: Option<&str>) -> Result<()> {
    let args = cargo_update_args(git, branch);
    println!("atualizando rege de {git}{}…", branch.map(|b| format!(" ({b})")).unwrap_or_default());
    let status = Command::new("cargo").args(&args).status();
    match status {
        Ok(s) if s.success() => {
            println!("✓ rege atualizado. rode `rege --version` pra conferir.");
            Ok(())
        }
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("falha ao rodar cargo (instalado? no PATH?): {e}");
            std::process::exit(1);
        }
    }
}

/// The `cargo install` argv for a self-update. Split out so the flag wiring is
/// unit-testable without shelling out.
fn cargo_update_args(git: &str, branch: Option<&str>) -> Vec<String> {
    let mut a = vec!["install".to_string(), "--git".to_string(), git.to_string(), "--force".to_string()];
    if let Some(b) = branch {
        a.push("--branch".to_string());
        a.push(b.to_string());
    }
    a
}

/// Instantiate a Session/Engine for `repo` and serve MCP over stdin/stdout.
fn mcp_serve(home: &Path, repo: &Path) -> Result<()> {
    let cfg = Config::load(Some(repo), home)?;
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
    fn cargo_update_args_defaults_to_forced_git_install() {
        assert_eq!(
            cargo_update_args(REGE_GIT_URL, None),
            vec!["install", "--git", REGE_GIT_URL, "--force"]
        );
    }

    #[test]
    fn cargo_update_args_appends_branch() {
        let a = cargo_update_args("https://x/y.git", Some("dev"));
        assert_eq!(a, vec!["install", "--git", "https://x/y.git", "--force", "--branch", "dev"]);
    }
}
