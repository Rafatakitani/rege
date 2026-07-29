//! Worker runs inside a detached tmux session. tmux gives us, for free: a
//! real PTY, persistence (survives app/ssh crash), live attach, and keystroke
//! injection for takeover. Output is teed to a log file via pipe-pane so we
//! can read it even after the pane exits.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const EXIT_PREFIX: &str = "__RG_EXIT_";
const EXIT_SUFFIX: &str = "__";
/// Printed by the command itself as its first output, so we can tell real
/// output from the shell's echo of the keystrokes we inject.
const START_MARKER: &str = "__RG_START__";

pub struct Tmux {
    pub session: String,
    pub logfile: PathBuf,
}

impl Tmux {
    pub fn new(session: &str, logdir: Option<&Path>) -> Result<Tmux> {
        let logdir = match logdir {
            Some(d) => d.to_path_buf(),
            None => std::env::temp_dir().join("rege-logs"),
        };
        std::fs::create_dir_all(&logdir)?;
        let logfile = logdir.join(format!("{}.log", session));
        std::fs::write(&logfile, "")?;
        Ok(Tmux { session: session.to_string(), logfile })
    }

    /// Start `command` inside a fresh detached session in `cwd`.
    pub fn start(&self, command: &str, cwd: &Path) -> Result<()> {
        run_tmux(&[
            "new-session",
            "-d",
            "-s",
            &self.session,
            "-x",
            "200",
            "-y",
            "50",
            "-c",
            cwd.to_str().ok_or_else(|| anyhow!("cwd invalido"))?,
            "sh",
        ])?;
        let pipe_cmd = format!("cat >> {}", shell_quote(&self.logfile));
        run_tmux(&["pipe-pane", "-o", "-t", &self.session, &pipe_cmd])?;
        // subshell so a command that calls `exit` doesn't kill our shell
        // before the sentinel is written. Both markers are assembled from
        // pieces at runtime so the shell's echo of this line — which contains
        // the whole worker prompt — can never look like a marker.
        let wrapped = format!(
            "printf '%s%s\\n' '__RG_' 'START__'; ( {} ); printf '\\n%s%s%s%s\\n' '__RG_' 'EXIT_' \"$?\" '__'",
            command
        );
        run_tmux(&["send-keys", "-t", &self.session, &wrapped, "Enter"])?;
        Ok(())
    }

    /// Inject keystrokes (used for redirect / takeover). Sends Enter after.
    pub fn send(&self, text: &str) -> Result<()> {
        run_tmux(&["send-keys", "-t", &self.session, text])?;
        run_tmux(&["send-keys", "-t", &self.session, "Enter"])?;
        Ok(())
    }

    /// Whether the tmux session still exists.
    pub fn alive(&self) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", &self.session])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// The command finished if the exit sentinel has been written.
    pub fn done(&self) -> bool {
        find_exit_code(&self.log_contents()).is_some()
    }

    pub fn exit_code(&self) -> Option<i32> {
        find_exit_code(&self.log_contents())
    }

    /// Full captured output, without the echoed launch line or the sentinel.
    pub fn output(&self) -> String {
        strip_sentinel(strip_launch_echo(&self.log_contents()))
    }

    /// Current visible pane snapshot (for live dashboards).
    pub fn snapshot(&self) -> String {
        Command::new("tmux")
            .args(["capture-pane", "-p", "-t", &self.session])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    /// Block until the command finishes (sentinel) or timeout. Returns true
    /// if it finished, false on timeout.
    pub fn wait(&self, timeout_secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        while !self.done() {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        true
    }

    pub fn kill(&self) -> Result<()> {
        let _ = run_tmux(&["kill-session", "-t", &self.session]);
        Ok(())
    }

    fn log_contents(&self) -> String {
        std::fs::read_to_string(&self.logfile).unwrap_or_default()
    }
}

/// Exit code from the sentinel line, skipping occurrences that are just the
/// echoed command text (`%s` where the code should be).
pub fn find_exit_code(log: &str) -> Option<i32> {
    let mut search_from = 0;
    while let Some(rel) = log[search_from..].find(EXIT_PREFIX) {
        let start = search_from + rel + EXIT_PREFIX.len();
        let rest = &log[start..];
        let Some(end) = rest.find(EXIT_SUFFIX) else { break };
        if let Ok(code) = rest[..end].parse() {
            return Some(code);
        }
        search_from = start + end + EXIT_SUFFIX.len();
    }
    None
}

/// The log from the start marker on. Before it sits the shell's echo of the
/// command we injected, which is keystrokes, not output. Logs written before
/// the marker existed (or captured mid-launch) come back untouched.
pub fn strip_launch_echo(log: &str) -> &str {
    match log.find(START_MARKER) {
        Some(i) => log[i + START_MARKER.len()..].trim_start_matches(['\r', '\n']),
        None => log,
    }
}

/// The log with real sentinel lines removed (echoed prefixes are kept).
pub fn strip_sentinel(log: &str) -> String {
    let mut out = String::new();
    let mut rest = log;
    loop {
        match rest.find(EXIT_PREFIX) {
            None => {
                out.push_str(rest);
                break;
            }
            Some(start) => {
                let after_prefix = &rest[start + EXIT_PREFIX.len()..];
                let matched = after_prefix.find(EXIT_SUFFIX).filter(|&end| {
                    let code = &after_prefix[..end];
                    !code.is_empty() && code.chars().all(|c| c.is_ascii_digit())
                });
                match matched {
                    None => {
                        // Not a real sentinel (e.g. the echoed command text
                        // itself) — keep it and keep scanning past the prefix.
                        out.push_str(&rest[..start + EXIT_PREFIX.len()]);
                        rest = after_prefix;
                    }
                    Some(end) => {
                        out.push_str(&rest[..start]);
                        let after_suffix = &after_prefix[end + EXIT_SUFFIX.len()..];
                        rest = after_suffix.trim_start_matches(['\n', '\r']);
                    }
                }
            }
        }
    }
    out
}

fn run_tmux(args: &[&str]) -> Result<()> {
    let output = Command::new("tmux").args(args).output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux {} failed: {}", args.first().unwrap_or(&""), err));
    }
    Ok(())
}

fn shell_quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmux_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    macro_rules! skip_if_no_tmux {
        () => {
            if !tmux_available() {
                eprintln!("skip: tmux not installed");
                return;
            }
        };
    }

    fn tmp_logdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-tmux-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn new_creates_logfile() {
        let logdir = tmp_logdir("new");
        let t = Tmux::new("rg-test-new", Some(&logdir)).unwrap();
        assert!(t.logfile.exists());
        assert_eq!(t.output(), "");
    }

    #[test]
    fn start_run_and_wait_for_exit() {
        skip_if_no_tmux!();
        let logdir = tmp_logdir("start");
        let session = format!("rg-test-start-{}", std::process::id());
        let t = Tmux::new(&session, Some(&logdir)).unwrap();
        t.start("echo hello-tmux; exit 0", &std::env::temp_dir()).unwrap();
        assert!(t.alive());
        assert!(t.wait(10));
        assert!(t.done());
        assert_eq!(t.exit_code(), Some(0));
        assert!(t.output().contains("hello-tmux"));
        assert!(find_exit_code(&t.output()).is_none());
        // The shell echoes the command we injected; that echo is keystrokes,
        // not output, and would otherwise dominate every status line.
        assert!(!t.output().contains("echo hello-tmux"), "launch echo leaked: {:?}", t.output());
        assert!(!t.output().contains(START_MARKER));
        t.kill().unwrap();
    }

    #[test]
    fn strip_launch_echo_cuts_everything_up_to_the_marker() {
        let log = "printf '%s%s\\n' '__RG_' 'START__'; ( work )\n__RG_START__\nreal output\n";
        assert_eq!(strip_launch_echo(log), "real output\n");
        // No marker (older log, or read mid-launch): nothing is dropped.
        assert_eq!(strip_launch_echo("plain\n"), "plain\n");
    }

    #[test]
    fn nonzero_exit_code_captured() {
        skip_if_no_tmux!();
        let logdir = tmp_logdir("exitcode");
        let session = format!("rg-test-exitcode-{}", std::process::id());
        let t = Tmux::new(&session, Some(&logdir)).unwrap();
        t.start("exit 7", &std::env::temp_dir()).unwrap();
        assert!(t.wait(10));
        assert_eq!(t.exit_code(), Some(7));
        t.kill().unwrap();
    }

    #[test]
    fn send_injects_keystrokes() {
        skip_if_no_tmux!();
        let logdir = tmp_logdir("send");
        let session = format!("rg-test-send-{}", std::process::id());
        let t = Tmux::new(&session, Some(&logdir)).unwrap();
        t.start("cat", &std::env::temp_dir()).unwrap();
        assert!(t.alive());
        t.send("echo from-send").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.output().contains("from-send"));
        t.kill().unwrap();
    }

    #[test]
    fn snapshot_returns_pane_text() {
        skip_if_no_tmux!();
        let logdir = tmp_logdir("snapshot");
        let session = format!("rg-test-snapshot-{}", std::process::id());
        let t = Tmux::new(&session, Some(&logdir)).unwrap();
        t.start("echo snap-marker; sleep 5", &std::env::temp_dir()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let snap = t.snapshot();
        assert!(snap.contains("snap-marker"));
        t.kill().unwrap();
    }

    #[test]
    fn kill_ends_session() {
        skip_if_no_tmux!();
        let logdir = tmp_logdir("kill");
        let session = format!("rg-test-kill-{}", std::process::id());
        let t = Tmux::new(&session, Some(&logdir)).unwrap();
        t.start("sleep 30", &std::env::temp_dir()).unwrap();
        assert!(t.alive());
        t.kill().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!t.alive());
    }

    #[test]
    fn wait_times_out_when_never_done() {
        skip_if_no_tmux!();
        let logdir = tmp_logdir("timeout");
        let session = format!("rg-test-timeout-{}", std::process::id());
        let t = Tmux::new(&session, Some(&logdir)).unwrap();
        t.start("sleep 30", &std::env::temp_dir()).unwrap();
        assert!(!t.wait(1));
        t.kill().unwrap();
    }
}
