//! regente — orquestrador multi-agente de IAs (Rust).
#![allow(dead_code)] // WIP: modules land incrementally

mod command;
mod config;
mod playbook;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
#[command(name = "regente", version, about = "Orquestrador multi-agente de IAs")]
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
        None => {
            // TUI (ratatui) chega no proximo bloco.
            eprintln!("regente: TUI em construcao. Use `regente exec \"<tarefa>\"` ou `regente doctor`.");
            Ok(())
        }
    }
}

/// Headless run: seed the master with the playbook + task, stream to stdout.
fn exec(cfg: &Config, task: &str) -> Result<()> {
    if task.trim().is_empty() {
        eprintln!("uso: regente exec \"<tarefa>\"");
        std::process::exit(2);
    }
    // exec = headless coding agent (like `codex exec`): runs the task directly.
    // The orchestrator playbook is for the interactive TUI, not exec.
    let yolo = cfg.sandbox.get("yolo").copied().unwrap_or(true);
    let mut a = command::argv(&cfg.master.cli, task, cfg.master.model.as_deref(), yolo)?;
    let bin = a.remove(0);
    let status = Command::new(bin).args(&a).status()?;
    std::process::exit(status.code().unwrap_or(1));
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
