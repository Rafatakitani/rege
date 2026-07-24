//! The stateful context the master operates on through MCP tools. Wraps the
//! Engine and tracks agents by id. All methods return `serde_json::Value`
//! (JSON-friendly) so the MCP layer can serialize them directly. Porta
//! `legacy/lib/rege/session.rb` + `legacy/lib/rege/pr.rb`.

use crate::agent::{Agent, State};
use crate::command;
use crate::config::Config;
use crate::engine::Engine;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command as Proc;

pub struct Session<'a> {
    repo: PathBuf,
    config: &'a Config,
    engine: Engine<'a>,
    counter: usize,
}

impl<'a> Session<'a> {
    pub fn new(repo: &Path, config: &'a Config) -> Self {
        Session { repo: repo.to_path_buf(), config, engine: Engine::new(repo, config), counter: 0 }
    }

    pub fn spawn_agent(
        &mut self,
        cli: &str,
        task: &str,
        model: Option<&str>,
        role: Option<&str>,
        command: Option<String>,
    ) -> Result<Value> {
        self.counter += 1;
        let name = format!("a{}", self.counter);
        let idx = self.engine.spawn(&name, cli, task, model, role, None, command)?;
        let agent = &self.engine.agents[idx];
        let out = json!({ "agent_id": name, "branch": agent.branch(), "state": state_str(agent.state()) });
        self.persist();
        Ok(out)
    }

    /// Snapshot the roster to `.rege-runs/run-<id>.json` so a crashed master's
    /// agents (their branches, worktrees, tmux sessions) stay discoverable and
    /// cleanable instead of vanishing with the in-memory engine.
    fn persist(&self) {
        let agents: Vec<Value> = self
            .engine
            .agents
            .iter()
            .map(|a| {
                json!({
                    "agent_id": a.name,
                    "cli": a.cli,
                    "model": a.model,
                    "role": a.role,
                    "branch": a.worktree.branch,
                    "worktree": a.worktree.path.to_string_lossy(),
                    "tmux": a.tmux.session,
                    "state": state_str(a.state()),
                })
            })
            .collect();
        let manifest = json!({
            "run": crate::agent::run_id(),
            "repo": self.repo.to_string_lossy(),
            "agents": agents,
        });
        let dir = self.repo.join(".rege-runs");
        if std::fs::create_dir_all(&dir).is_ok() {
            let path = dir.join(format!("run-{}.json", crate::agent::run_id()));
            if let Ok(bytes) = serde_json::to_vec_pretty(&manifest) {
                let _ = std::fs::write(path, bytes);
            }
        }
    }

    pub fn list_agents(&mut self) -> Value {
        let agents: Vec<Value> = self
            .engine
            .agents
            .iter_mut()
            .map(|a| {
                json!({
                    "agent_id": a.name,
                    "cli": a.cli,
                    "model": a.model,
                    "role": a.role,
                    "state": state_str(a.refresh()),
                })
            })
            .collect();
        json!({ "agents": agents })
    }

    pub fn agent_status(&mut self, agent_id: &str) -> Value {
        self.with_agent_mut(agent_id, |a| {
            let state = a.refresh();
            if state == State::Done {
                a.commit(None);
            }
            json!({ "agent_id": agent_id, "state": state_str(state) })
        })
    }

    /// Block until the agent finishes (or timeout), then commit its work.
    pub fn wait_agent(&mut self, agent_id: &str, timeout: Option<u64>) -> Value {
        let timeout = timeout.unwrap_or(300);
        let out = self.with_agent_mut(agent_id, |a| {
            let state = a.wait(timeout);
            if state == State::Done {
                a.commit(None);
            }
            json!({ "agent_id": agent_id, "state": state_str(state) })
        });
        self.persist();
        out
    }

    pub fn read_output(&self, agent_id: &str) -> Value {
        self.with_agent(agent_id, |a| json!({ "agent_id": agent_id, "output": a.output() }))
    }

    pub fn send_message(&mut self, agent_id: &str, text: &str) -> Value {
        self.with_agent_mut(agent_id, |a| {
            let sent = a.send(text).is_ok();
            json!({ "agent_id": agent_id, "sent": sent })
        })
    }

    pub fn kill_agent(&mut self, agent_id: &str) -> Value {
        let out = self.with_agent_mut(agent_id, |a| {
            let killed = a.tmux.kill().is_ok();
            json!({ "agent_id": agent_id, "killed": killed })
        });
        self.persist();
        out
    }

    pub fn diff_agent(&mut self, agent_id: &str) -> Value {
        self.with_agent_mut(agent_id, |a| {
            if a.refresh() == State::Done {
                a.commit(None);
            }
            let diff = a.diff().unwrap_or_default();
            json!({ "agent_id": agent_id, "diff": diff })
        })
    }

    pub fn review(&self, agent_ids: &[String]) -> Value {
        let diffs: Vec<Value> = agent_ids
            .iter()
            .map(|id| {
                let a = self.find(id);
                json!({
                    "agent_id": id,
                    "branch": a.map(|a| a.branch()),
                    "diff": a.and_then(|a| a.diff().ok()),
                })
            })
            .collect();
        json!({ "review": diffs })
    }

    /// Run the configured verify command inside an agent's worktree.
    pub fn run_tests(&mut self, agent_id: &str) -> Value {
        let Some(cmd) = self.config.verify.get("command") else {
            return json!({ "skipped": true, "reason": "sem verify.command configurado" });
        };
        let cmd = cmd.clone();
        self.with_agent_mut(agent_id, |a| {
            let out = Proc::new("sh").arg("-c").arg(&cmd).current_dir(&a.worktree.path).output();
            match out {
                Ok(o) => json!({
                    "agent_id": agent_id,
                    "passed": o.status.success(),
                    "output": String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
                }),
                Err(e) => json!({ "agent_id": agent_id, "passed": false, "output": e.to_string() }),
            }
        })
    }

    /// Ask a stronger model a one-shot question (escalation without spawning a worker).
    pub fn consult(&self, question: &str, model: Option<&str>, cli: Option<&str>) -> Result<Value> {
        let model = model.unwrap_or("opus");
        let cli = cli.unwrap_or("claude");
        let argv = command::argv(cli, question, Some(model), false)?;
        let out = Proc::new(&argv[0]).args(&argv[1..]).current_dir(&self.repo).output()?;
        Ok(json!({
            "model": model,
            "answer": String::from_utf8_lossy(&out.stdout).trim().to_string(),
            "ok": out.status.success(),
        }))
    }

    pub fn open_pr(&self, branch: &str, title: &str, body: &str) -> Result<Value> {
        if self.config.pr.get("provider").map(String::as_str) == Some("github")
            && gh_available()
            && has_remote(&self.repo)?
        {
            let out = Proc::new("gh")
                .args(["pr", "create", "--head", branch, "--title", title, "--body", body])
                .current_dir(&self.repo)
                .output()?;
            if !out.status.success() {
                anyhow::bail!("gh pr create falhou: {}", String::from_utf8_lossy(&out.stderr));
            }
            let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(json!({ "mode": "pr", "ref": url }))
        } else {
            let path = write_patch(&self.repo, branch)?;
            Ok(json!({ "mode": "patch", "ref": path.to_string_lossy() }))
        }
    }

    fn find(&self, id: &str) -> Option<&Agent> {
        self.engine.agents.iter().find(|a| a.name == id)
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut Agent> {
        self.engine.agents.iter_mut().find(|a| a.name == id)
    }

    fn with_agent<F: FnOnce(&Agent) -> Value>(&self, id: &str, f: F) -> Value {
        match self.find(id) {
            Some(a) => f(a),
            None => json!({ "error": format!("agente inexistente: {id}") }),
        }
    }

    fn with_agent_mut<F: FnOnce(&mut Agent) -> Value>(&mut self, id: &str, f: F) -> Value {
        match self.find_mut(id) {
            Some(a) => f(a),
            None => json!({ "error": format!("agente inexistente: {id}") }),
        }
    }
}

fn state_str(state: State) -> &'static str {
    match state {
        State::Pending => "pending",
        State::Running => "running",
        State::Done => "done",
        State::Failed => "failed",
        State::Timeout => "timeout",
    }
}

fn gh_available() -> bool {
    Proc::new("which")
        .arg("gh")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn has_remote(repo: &Path) -> Result<bool> {
    let out = Proc::new("git").arg("-C").arg(repo).arg("remote").output()?;
    Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

fn default_branch(repo: &Path) -> Result<String> {
    let out = Proc::new("git").arg("-C").arg(repo).args(["rev-parse", "--abbrev-ref", "HEAD"]).output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_patch(repo: &Path, branch: &str) -> Result<PathBuf> {
    let base = default_branch(repo)?;
    let out = Proc::new("git").arg("-C").arg(repo).arg("diff").arg(format!("{base}...{branch}")).output()?;
    if !out.status.success() {
        anyhow::bail!("git diff falhou: {}", String::from_utf8_lossy(&out.stderr));
    }
    let dir = repo.join(".rege-runs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.patch", branch.replace('/', "-")));
    std::fs::write(&path, out.stdout)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    // `run_id()` is process-global, so two tests that both spawn agent "a1"
    // would fight over the same `rege-<run>-a1` tmux/worktree. Serialize them
    // (a real master never reuses ids within a run; only the test harness does).
    static SPAWN_LOCK: Mutex<()> = Mutex::new(());

    fn init_repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-session-test-{}-{}", std::process::id(), name));
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
        let status = Proc::new("git").arg("-C").arg(dir).args(args).status().unwrap();
        assert!(status.success(), "git {:?} falhou", args);
    }

    fn tmux_available() -> bool {
        Proc::new("tmux")
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
                eprintln!("skip: tmux nao instalado");
                return;
            }
        };
    }

    #[test]
    fn list_agents_empty_by_default() {
        let repo = init_repo("list-empty");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        assert_eq!(session.list_agents(), json!({ "agents": [] }));
    }

    #[test]
    fn agent_status_unknown_returns_error() {
        let repo = init_repo("status-unknown");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        let v = session.agent_status("nope");
        assert_eq!(v["error"], json!("agente inexistente: nope"));
    }

    #[test]
    fn read_output_unknown_returns_error() {
        let repo = init_repo("output-unknown");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let v = session.read_output("nope");
        assert!(v["error"].is_string());
    }

    #[test]
    fn review_with_unknown_agent_ids_yields_nulls() {
        let repo = init_repo("review-unknown");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let v = session.review(&["ghost".to_string()]);
        let review = v["review"].as_array().unwrap();
        assert_eq!(review.len(), 1);
        assert_eq!(review[0]["agent_id"], json!("ghost"));
        assert!(review[0]["branch"].is_null());
    }

    #[test]
    fn run_tests_skips_without_verify_command() {
        let repo = init_repo("run-tests-skip");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        let v = session.run_tests("nope");
        assert_eq!(v["skipped"], json!(true));
    }

    #[test]
    fn spawn_agent_lifecycle_ids_wait_and_diff() {
        skip_if_no_tmux!();
        let _guard = SPAWN_LOCK.lock().unwrap();
        let repo = init_repo("spawn-lifecycle");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        let a1 = session
            .spawn_agent("claude", "t", None, None, Some("echo hi >> out.txt; exit 0".to_string()))
            .unwrap();
        let a2 = session
            .spawn_agent("claude", "t", None, None, Some("echo ok; exit 0".to_string()))
            .unwrap();
        assert_eq!(a1["agent_id"], json!("a1"));
        assert_eq!(a2["agent_id"], json!("a2"));

        let id = a1["agent_id"].as_str().unwrap().to_string();
        let status = session.wait_agent(&id, Some(10));
        assert_eq!(status["state"], json!("done"));
        let diff = session.diff_agent(&id);
        assert!(diff["diff"].as_str().unwrap().contains("out.txt"));
        session.engine.shutdown();
    }

    #[test]
    fn spawn_writes_run_manifest_with_scoped_branch() {
        skip_if_no_tmux!();
        let _guard = SPAWN_LOCK.lock().unwrap();
        let repo = init_repo("manifest");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        session
            .spawn_agent("claude", "t", None, None, Some("exit 0".to_string()))
            .unwrap();
        let path = repo.join(".rege-runs").join(format!("run-{}.json", crate::agent::run_id()));
        assert!(path.exists(), "manifest nao escrito");
        let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["agents"][0]["agent_id"], json!("a1"));
        let branch = v["agents"][0]["branch"].as_str().unwrap();
        assert!(branch.starts_with("rege/") && branch.ends_with("-a1"), "branch nao scopado: {branch}");
        session.engine.shutdown();
    }

    #[test]
    fn open_pr_falls_back_to_patch_without_remote() {
        let repo = init_repo("open-pr");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let base = default_branch(&repo).unwrap();
        run(&repo, &["checkout", "-b", "rege/x"]);
        fs::write(repo.join("out.txt"), "hi\n").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "work"]);
        run(&repo, &["checkout", &base]);
        let result = session.open_pr("rege/x", "titulo", "corpo").unwrap();
        assert_eq!(result["mode"], json!("patch"));
        let path = PathBuf::from(result["ref"].as_str().unwrap());
        assert!(path.exists());
    }
}
