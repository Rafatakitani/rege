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

use crate::config::Rtk;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

static CONFIGURED: OnceLock<Rtk> = OnceLock::new();

/// Hands the loaded `[rtk]` config to the module, once per process. Called at
/// startup before anything spawns; without it the module still works and just
/// falls back to autodetection.
pub fn configure(settings: &Rtk) {
    let _ = CONFIGURED.set(settings.clone());
}

fn settings() -> Rtk {
    CONFIGURED.get().cloned().unwrap_or_default()
}

/// True when diffs should be piped through `rtk`.
pub fn enabled() -> bool {
    resolve(env_override(), settings().enabled, on_path())
}

/// The precedence, in one place: `REGE_RTK` beats `config.yml` beats
/// autodetection. Each layer is more specific — and more temporary — than the
/// one under it, so the narrower scope wins.
fn resolve(env: Option<bool>, config: Option<bool>, on_path: bool) -> bool {
    env.or(config).unwrap_or(on_path)
}

/// Argv that installs rtk's own hook inside a worker's worktree, or `None`
/// when this worker shouldn't get one. Opt-in only: writing hook files into a
/// worktree is intrusive, so autodetection never triggers it — `hook_workers`
/// has to be set, and the worker's CLI has to be listed.
pub fn worker_hook_argv(cli: &str) -> Option<Vec<String>> {
    hook_argv(&settings(), cli, enabled())
}

fn hook_argv(s: &Rtk, cli: &str, enabled: bool) -> Option<Vec<String>> {
    if !s.hook_workers || !enabled || !s.clis.iter().any(|c| c == cli) {
        return None;
    }
    let mut argv = vec!["rtk".to_string()];
    argv.extend(s.init_args.iter().cloned());
    Some(argv)
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
    fn env_beats_config_beats_autodetection() {
        // Env wins whatever is under it.
        assert!(resolve(Some(true), Some(false), false));
        assert!(!resolve(Some(false), Some(true), true));
        // No env: the config file decides.
        assert!(resolve(None, Some(true), false));
        assert!(!resolve(None, Some(false), true));
        // Neither: fall back to "is the binary there?".
        assert!(resolve(None, None, true));
        assert!(!resolve(None, None, false));
    }

    #[test]
    fn worker_hook_is_opt_in_not_autodetected() {
        let mut s = Rtk { hook_workers: false, ..Rtk::default() };
        // Binary present and compression on, but hook_workers off: no hook.
        assert_eq!(hook_argv(&s, "claude", true), None);

        s.hook_workers = true;
        assert_eq!(
            hook_argv(&s, "claude", true),
            Some(vec!["rtk".into(), "init".into(), "--hook-only".into()])
        );
        // rtk itself unavailable/disabled: nothing to install.
        assert_eq!(hook_argv(&s, "claude", false), None);
        // CLI outside the configured list stays untouched.
        assert_eq!(hook_argv(&s, "codex", true), None);
    }

    #[test]
    fn worker_hook_argv_follows_configured_init_args() {
        let s = Rtk {
            hook_workers: true,
            clis: vec!["codex".into()],
            init_args: vec!["init".into(), "--codex".into()],
            ..Rtk::default()
        };
        assert_eq!(
            hook_argv(&s, "codex", true),
            Some(vec!["rtk".into(), "init".into(), "--codex".into()])
        );
    }

    #[test]
    fn empty_args_still_names_the_binary() {
        assert_eq!(argv_with(true, &[]), vec!["rtk", "git"]);
        assert_eq!(argv_with(false, &[]), vec!["git"]);
    }
}
