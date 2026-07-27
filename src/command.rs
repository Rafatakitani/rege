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

/// Extra flags for a call that wants *prose back*, not work done: the model
/// answers from the prompt alone. Without this, a CLI handed "write the
/// contents of AGENTS.md" reaches for its `Write` tool and blocks on a
/// permission prompt — whose text then lands on stdout as if it were the
/// answer. Only `claude` exposes a flag for this; the rest return empty, so
/// callers stay best-effort rather than pretending to a guarantee.
pub fn text_only_flags(cli: &str) -> Vec<String> {
    match cli {
        "claude" => ["--disallowedTools", "Write", "Edit", "NotebookEdit", "Bash", "Read", "Glob", "Grep", "Task"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
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
    fn text_only_disables_the_tools_that_hijack_a_prose_answer() {
        let f = text_only_flags("claude");
        assert_eq!(f.first().map(String::as_str), Some("--disallowedTools"));
        // Write is the one that actually blocked on a permission prompt.
        assert!(f.contains(&"Write".to_string()));
        assert!(text_only_flags("codex").is_empty(), "no known flag: better empty than a guess");
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
