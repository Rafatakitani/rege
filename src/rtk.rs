//! Optional integration with `rtk` (https://github.com/rtk-ai/rtk), a CLI proxy
//! that compresses command output before it reaches an LLM (60-90% fewer tokens).
//!
//! Only output that ends up **inside the master's context** is routed through
//! `rtk`: agent diffs (`diff_agent`, `review`). Anything consumed by a machine —
//! the `.patch` files written by `open_pr`, git plumbing like `rev-parse` — stays
//! raw, because a condensed diff is not appliable.
//!
//! Detection is automatic (`rtk` on PATH). Override with `REGE_RTK`:
//! `0`/`off`/`false` forces raw, `1`/`on`/`true` forces the wrapper even if the
//! probe fails.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// True when diffs should be piped through `rtk`.
pub fn enabled() -> bool {
    match env_override() {
        Some(v) => v,
        None => on_path(),
    }
}

/// `git <args>` prefixed with `rtk` when enabled. `rtk git` forwards native git
/// flags (including `-C <repo>`) and preserves the exit status.
pub fn git_argv(args: &[&str]) -> Vec<String> {
    argv_with(enabled(), args)
}

fn argv_with(enabled: bool, args: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = if enabled {
        vec!["rtk".into(), "git".into()]
    } else {
        vec!["git".into()]
    };
    argv.extend(args.iter().map(|a| a.to_string()));
    argv
}

fn env_override() -> Option<bool> {
    let raw = std::env::var("REGE_RTK").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "0" | "off" | "false" | "no" => Some(false),
        "1" | "on" | "true" | "yes" => Some(true),
        _ => None,
    }
}

/// Probed once per process — the binary does not appear mid-run.
fn on_path() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        Command::new("rtk")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_when_enabled() {
        let a = argv_with(true, &["-C", "/repo", "diff", "main...x"]);
        assert_eq!(a, vec!["rtk", "git", "-C", "/repo", "diff", "main...x"]);
    }

    #[test]
    fn stays_raw_when_disabled() {
        let a = argv_with(false, &["-C", "/repo", "diff", "main...x"]);
        assert_eq!(a, vec!["git", "-C", "/repo", "diff", "main...x"]);
    }

    #[test]
    fn empty_args_still_names_the_binary() {
        assert_eq!(argv_with(true, &[]), vec!["rtk", "git"]);
        assert_eq!(argv_with(false, &[]), vec!["git"]);
    }
}
