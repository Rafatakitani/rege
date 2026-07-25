//! Builds the headless, auto-approving invocation for each supported CLI.
//! Everything runs confined to the agent's worktree cwd; yolo flags remove
//! interactive prompts.

use anyhow::{bail, Result};

/// CLIs the roster knows how to invoke. Anything outside this set has no
/// `argv` recipe, so the `/agents` picker refuses to connect it.
pub const KNOWN_CLIS: &[&str] = &["claude", "codex", "gemini", "opencode"];

pub fn argv(cli: &str, task: &str, model: Option<&str>, yolo: bool) -> Result<Vec<String>> {
    let mut a: Vec<String> = Vec::new();
    match cli {
        "claude" => {
            a.push("claude".into());
            a.push("-p".into());
            a.push(task.into());
            if let Some(m) = model {
                a.push("--model".into());
                a.push(m.into());
            }
            if yolo {
                a.push("--dangerously-skip-permissions".into());
            }
        }
        "codex" => {
            a.push("codex".into());
            a.push("exec".into());
            if let Some(m) = model {
                a.push("-m".into());
                a.push(m.into());
            }
            if yolo {
                a.push("--dangerously-bypass-approvals-and-sandbox".into());
            }
            a.push(task.into());
        }
        "gemini" => {
            a.push("gemini".into());
            a.push("-p".into());
            a.push(task.into());
            if let Some(m) = model {
                a.push("-m".into());
                a.push(m.into());
            }
            if yolo {
                a.push("--yolo".into());
            }
        }
        "opencode" => {
            a.push("opencode".into());
            a.push("run".into());
            if let Some(m) = model {
                a.push("--model".into());
                a.push(m.into());
            }
            a.push(task.into());
        }
        other => bail!("CLI desconhecido no roster: {other}"),
    }
    Ok(a)
}

/// Trivial liveness probe for health checks.
pub fn probe(cli: &str) -> Result<Vec<String>> {
    argv(cli, "reply with OK", None, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_headless_with_model_and_yolo() {
        let a = argv("claude", "fix bug", Some("opus"), true).unwrap();
        assert_eq!(a, vec!["claude", "-p", "fix bug", "--model", "opus", "--dangerously-skip-permissions"]);
    }

    #[test]
    fn codex_exec() {
        let a = argv("codex", "t", Some("gpt"), true).unwrap();
        assert_eq!(a, vec!["codex", "exec", "-m", "gpt", "--dangerously-bypass-approvals-and-sandbox", "t"]);
    }

    #[test]
    fn gemini_yolo() {
        let a = argv("gemini", "t", None, true).unwrap();
        assert_eq!(a, vec!["gemini", "-p", "t", "--yolo"]);
    }

    #[test]
    fn opencode_run() {
        let a = argv("opencode", "t", Some("x"), true).unwrap();
        assert_eq!(a, vec!["opencode", "run", "--model", "x", "t"]);
    }

    #[test]
    fn unknown_cli_errors() {
        assert!(argv("nope", "t", None, true).is_err());
    }
}
