//! Drives the master model headless in streaming mode on a background
//! thread, translating `stream-json` lines into `stream::Event`s sent over
//! an mpsc channel so the TUI can render the conversation as it arrives.
//! Porta `legacy/lib/rege/master_driver.rb`. Multi-turn is done by
//! re-spawning the process with `--resume <session_id>` (no long-lived
//! stdin protocol).

use crate::stream::{self, Event};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Where the running master process is parked while a turn is in flight, so
/// whoever started the turn can kill it from another thread (Ctrl-C).
pub type ChildSlot = Arc<Mutex<Option<Child>>>;

pub fn child_slot() -> ChildSlot {
    Arc::new(Mutex::new(None))
}

/// Kills the master if a turn is running. No-op on an empty slot.
pub fn kill(slot: &ChildSlot) {
    let taken = slot.lock().ok().and_then(|mut g| g.take());
    if let Some(mut child) = taken {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Spawns one turn of the master model and streams parsed events back over
/// `tx`. Only `cli == "claude"` is supported; anything else sends a single
/// error-shaped Text event and returns immediately.
pub fn spawn_turn(
    cli: &str,
    model: Option<&str>,
    repo: &str,
    playbook: Option<&str>,
    task: &str,
    session_id: Option<String>,
    tx: Sender<Event>,
    slot: ChildSlot,
) {
    if cli != "claude" {
        let _ = tx.send(Event::Text(format!(
            "chat streaming so suporta claude (cli atual: {cli})"
        )));
        return;
    }

    let argv = build_argv(model, repo, playbook, task, session_id.as_deref());
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).current_dir(repo).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Event::Text(format!("could not start the master: {e}")));
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    // Park the child where the TUI can reach it: an interrupt kills it from
    // there, and this thread just sees stdout hit EOF.
    if let Ok(mut g) = slot.lock() {
        *g = Some(child);
    }
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        for event in stream::parse_line(&json) {
            if tx.send(event).is_err() {
                kill(&slot);
                return;
            }
        }
    }
    let reaped = slot.lock().ok().and_then(|mut g| g.take());
    if let Some(mut child) = reaped {
        let _ = child.wait();
    }
}

fn build_argv(
    model: Option<&str>,
    repo: &str,
    playbook: Option<&str>,
    task: &str,
    session_id: Option<&str>,
) -> Vec<String> {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "rege".into());
    let mcp = serde_json::json!({
        "mcpServers": { "rege": {
            "command": exe,
            "args": ["mcp-serve", "--repo", repo]
        }}
    })
    .to_string();

    let seed = match (session_id, playbook) {
        (None, Some(playbook)) => format!("{playbook}\n\nTask: {task}"),
        _ => task.to_string(),
    };

    let mut argv: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        seed,
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--dangerously-skip-permissions".into(),
        "--mcp-config".into(),
        mcp,
    ];
    if let Some(session_id) = session_id {
        argv.push("--resume".into());
        argv.push(session_id.to_string());
    }
    if let Some(model) = model {
        argv.push("--model".into());
        argv.push(model.to_string());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_argv_first_turn_includes_playbook_and_no_resume() {
        let argv = build_argv(None, "/repo", Some("PLAYBOOK"), "do something", None);
        assert!(argv.contains(&"PLAYBOOK\n\nTask: do something".to_string()));
        assert!(!argv.contains(&"--resume".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn build_argv_resumed_turn_uses_session_id_not_playbook() {
        let argv = build_argv(None, "/repo", Some("PLAYBOOK"), "continua", Some("sess-1"));
        assert!(argv.contains(&"continua".to_string()));
        assert!(!argv.iter().any(|a| a.contains("PLAYBOOK")));
        let idx = argv.iter().position(|a| a == "--resume").expect("has --resume");
        assert_eq!(argv[idx + 1], "sess-1");
    }

    #[test]
    fn build_argv_includes_model_when_set() {
        let argv = build_argv(Some("opus"), "/repo", None, "x", None);
        let idx = argv.iter().position(|a| a == "--model").expect("has --model");
        assert_eq!(argv[idx + 1], "opus");
    }
}
