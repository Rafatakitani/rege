//! A single worker: a CLI+model doing a task, isolated in a worktree, running
//! inside a tmux session. Wraps lifecycle + status.

use crate::command;
use crate::config::Config;
use crate::rtk;
use crate::tmux::Tmux;
use crate::worktree::Worktree;
use anyhow::Result;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// A token unique to this master process, computed once. Every agent's tmux
/// session, worktree path and branch are namespaced with it, so leftovers from
/// a crashed run can never collide with a fresh one (the bug that stalled a1/a2).
pub fn run_id() -> &'static str {
    static RUN: OnceLock<String> = OnceLock::new();
    RUN.get_or_init(|| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("{:x}{:x}", std::process::id(), secs & 0xff_ffff)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Running,
    Done,
    Failed,
    Timeout,
}

pub struct Agent {
    pub name: String,
    pub cli: String,
    pub model: Option<String>,
    pub task: String,
    pub role: String,
    pub worktree: Worktree,
    pub tmux: Tmux,
    command: Option<String>,
    yolo: bool,
    state: State,
}

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo: &Path,
        name: &str,
        cli: &str,
        task: &str,
        model: Option<&str>,
        role: Option<&str>,
        config: Option<&Config>,
        base: Option<&str>,
        command: Option<String>,
    ) -> Result<Self> {
        let yolo = config
            .and_then(|c| c.sandbox.get("yolo").copied())
            .unwrap_or(true);
        let prefix = config.and_then(|c| c.pr.get("branch_prefix").map(String::as_str));
        // Namespace the isolation resources with the run token; `name` (a1, a2…)
        // stays the user-facing id, but tmux/worktree/branch get a unique scope.
        let scoped = format!("{}-{}", run_id(), name);
        let worktree = Worktree::new(repo, &scoped, prefix, base, None)?;
        let tmux = Tmux::new(&format!("rege-{scoped}"), None)?;
        Ok(Agent {
            name: name.to_string(),
            cli: cli.to_string(),
            model: model.map(str::to_string),
            task: task.to_string(),
            role: role.unwrap_or("worker").to_string(),
            worktree,
            tmux,
            command,
            yolo,
            state: State::Pending,
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn start(&mut self) -> Result<()> {
        self.worktree.create()?;
        self.install_rtk_hook();
        let cmd = self.build_command()?;
        self.tmux.start(&cmd, &self.worktree.path)?;
        self.state = State::Running;
        Ok(())
    }

    /// Lets `rtk` install its own hook inside the worktree, so the worker's
    /// bash output gets compressed before it reaches that worker's model.
    /// Best-effort: rege never learns rtk's hook format, and a failure here
    /// only costs tokens — the worker runs either way.
    fn install_rtk_hook(&self) {
        let Some(argv) = rtk::worker_hook_argv(&self.cli) else {
            return;
        };
        let _ = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&self.worktree.path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    fn build_command(&self) -> Result<String> {
        if let Some(c) = &self.command {
            return Ok(c.clone());
        }
        let argv = command::argv(&self.cli, &self.task, self.model.as_deref(), self.yolo)?;
        Ok(shell_join(&argv))
    }

    /// Poll status; returns the current state.
    pub fn refresh(&mut self) -> State {
        if self.state != State::Running {
            return self.state;
        }
        if !self.tmux.done() {
            return self.state;
        }
        self.state = match self.tmux.exit_code() {
            Some(0) => State::Done,
            _ => State::Failed,
        };
        self.state
    }

    pub fn wait(&mut self, timeout_secs: u64) -> State {
        if !self.tmux.wait(timeout_secs) {
            self.state = State::Timeout;
            return State::Timeout;
        }
        self.refresh()
    }

    pub fn output(&self) -> String {
        self.tmux.output()
    }

    pub fn snapshot(&self) -> String {
        self.tmux.snapshot()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.tmux.exit_code()
    }

    pub fn send(&self, text: &str) -> Result<()> {
        self.tmux.send(text)
    }

    /// Commit the agent's work so the branch carries a diff for review.
    /// Errors (e.g. nothing to commit) are swallowed, mirroring the legacy behavior.
    pub fn commit(&self, message: Option<&str>) {
        let msg = message
            .map(str::to_string)
            .unwrap_or_else(|| format!("rege: {}", self.name));
        let _ = self.worktree.commit_all(&msg);
    }

    pub fn diff(&self) -> Result<String> {
        self.worktree.diff()
    }

    pub fn branch(&self) -> &str {
        &self.worktree.branch
    }

    /// Swap in a fresh tmux session and restart the same command (used on retry).
    pub fn restart(&mut self) -> Result<()> {
        let cmd = self.build_command()?;
        let tmux = Tmux::new(&format!("rege-{}-retry", self.worktree.name), None)?;
        tmux.start(&cmd, &self.worktree.path)?;
        self.tmux = tmux;
        self.state = State::Running;
        Ok(())
    }

    pub fn cleanup(&self) -> Result<()> {
        if self.tmux.alive() {
            let _ = self.tmux.kill();
        }
        self.worktree.remove(true)
    }
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:@%".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    fn init_repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-agent-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        run(&d, &["init", "-q"]);
        run(&d, &["config", "user.email", "test@example.com"]);
        run(&d, &["config", "user.name", "Test"]);
        fs::write(d.join("README.md"), "hello\n").unwrap();
        run(&d, &["add", "-A"]);
        run(&d, &["commit", "-q", "-m", "initial"]);
        d
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git").arg("-C").arg(dir).args(args).status().unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

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

    #[test]
    fn new_starts_pending_with_worktree_and_tmux() {
        let repo = init_repo("new");
        let agent = Agent::new(&repo, "a1", "claude", "faz X", None, None, None, None, None).unwrap();
        assert_eq!(agent.state(), State::Pending);
        assert_eq!(agent.role, "worker");
        // Branch is run-scoped (rege/<run>-a1) so crashed runs never collide.
        assert!(agent.worktree.branch.starts_with("rege/"), "got {}", agent.worktree.branch);
        assert!(agent.worktree.branch.ends_with("-a1"), "got {}", agent.worktree.branch);
        assert!(agent.tmux.session.starts_with("rege-") && agent.tmux.session.ends_with("-a1"));
    }

    #[test]
    fn build_command_uses_override_when_present() {
        let repo = init_repo("override");
        let agent = Agent::new(
            &repo,
            "a2",
            "claude",
            "faz X",
            None,
            None,
            None,
            None,
            Some("echo hi".to_string()),
        )
        .unwrap();
        assert_eq!(agent.build_command().unwrap(), "echo hi");
    }

    #[test]
    fn build_command_shell_quotes_task() {
        let repo = init_repo("quote");
        let agent = Agent::new(&repo, "a3", "claude", "faz X e Y", None, None, None, None, None).unwrap();
        let cmd = agent.build_command().unwrap();
        assert!(cmd.contains("'faz X e Y'"));
    }

    #[test]
    fn start_wait_done_and_cleanup() {
        skip_if_no_tmux!();
        let repo = init_repo("lifecycle");
        let mut agent = Agent::new(
            &repo,
            "a4",
            "claude",
            "t",
            None,
            None,
            None,
            None,
            Some("echo ok; exit 0".to_string()),
        )
        .unwrap();
        agent.start().unwrap();
        assert_eq!(agent.state(), State::Running);
        let final_state = agent.wait(10);
        assert_eq!(final_state, State::Done);
        assert!(agent.output().contains("ok"));
        agent.commit(None);
        agent.cleanup().unwrap();
        assert!(!agent.worktree.exists());
    }

    #[test]
    fn failing_command_yields_failed_state() {
        skip_if_no_tmux!();
        let repo = init_repo("failed");
        let mut agent = Agent::new(
            &repo,
            "a5",
            "claude",
            "t",
            None,
            None,
            None,
            None,
            Some("exit 3".to_string()),
        )
        .unwrap();
        agent.start().unwrap();
        let final_state = agent.wait(10);
        assert_eq!(final_state, State::Failed);
        agent.cleanup().unwrap();
    }

    #[test]
    fn wait_times_out_when_never_done() {
        skip_if_no_tmux!();
        let repo = init_repo("timeout");
        let mut agent = Agent::new(
            &repo,
            "a6",
            "claude",
            "t",
            None,
            None,
            None,
            None,
            Some("sleep 30".to_string()),
        )
        .unwrap();
        agent.start().unwrap();
        let final_state = agent.wait(1);
        assert_eq!(final_state, State::Timeout);
        agent.cleanup().unwrap();
    }

    #[test]
    fn restart_swaps_tmux_session_and_resets_state() {
        skip_if_no_tmux!();
        let repo = init_repo("restart");
        let mut agent = Agent::new(
            &repo,
            "a7",
            "claude",
            "t",
            None,
            None,
            None,
            None,
            Some("sleep 30".to_string()),
        )
        .unwrap();
        agent.start().unwrap();
        agent.tmux.kill().unwrap();
        agent.command = Some("echo restarted; exit 0".to_string());
        agent.restart().unwrap();
        assert_eq!(agent.state(), State::Running);
        let final_state = agent.wait(10);
        assert_eq!(final_state, State::Done);
        assert!(agent.output().contains("restarted"));
        agent.cleanup().unwrap();
    }
}
