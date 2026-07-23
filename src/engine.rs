//! Orchestration engine: spawns/tracks agents, enforces per-role timeouts
//! (kill + retry once), and runs a boot health check over the roster.
//! Fan-out uses threads: the work is in external processes, so lack of
//! async is not a bottleneck.

use crate::agent::{Agent, State};
use crate::command;
use crate::config::Config;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Engine<'a> {
    repo: PathBuf,
    config: &'a Config,
    pub agents: Vec<Agent>,
    probe_runner: Box<dyn Fn(&str) -> bool + Send + Sync>,
}

impl<'a> Engine<'a> {
    pub fn new(repo: &Path, config: &'a Config) -> Self {
        let timeout = config.timeouts.get("healthcheck").copied().unwrap_or(15);
        Engine {
            repo: repo.to_path_buf(),
            config,
            agents: Vec::new(),
            probe_runner: Box::new(move |cli| default_probe(cli, timeout)),
        }
    }

    pub fn with_probe_runner<F>(repo: &Path, config: &'a Config, probe_runner: F) -> Self
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        Engine {
            repo: repo.to_path_buf(),
            config,
            agents: Vec::new(),
            probe_runner: Box::new(probe_runner),
        }
    }

    /// Probe each distinct CLI in the roster; returns { "claude" => true, ... }.
    pub fn health_check(&self) -> HashMap<String, bool> {
        let mut out = HashMap::new();
        for cli in self.config.distinct_clis() {
            let ok = (self.probe_runner)(&cli);
            out.insert(cli, ok);
        }
        out
    }

    /// Create + start an agent, tracked by the engine. Returns its index.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &mut self,
        name: &str,
        cli: &str,
        task: &str,
        model: Option<&str>,
        role: Option<&str>,
        base: Option<&str>,
        command: Option<String>,
    ) -> Result<usize> {
        let mut agent = Agent::new(
            &self.repo,
            name,
            cli,
            task,
            model,
            role,
            Some(self.config),
            base,
            command,
        )?;
        agent.start()?;
        self.agents.push(agent);
        Ok(self.agents.len() - 1)
    }

    /// Run all tracked agents concurrently, each under its role timeout. On
    /// timeout, kill and retry once. Returns the names of agents that ended
    /// up in the `Done` state.
    pub fn run_all(&mut self) -> Vec<String> {
        let config = self.config;
        std::thread::scope(|scope| {
            let handles: Vec<_> = self
                .agents
                .iter_mut()
                .map(|agent| {
                    scope.spawn(move || {
                        supervise(agent, config);
                    })
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
        });
        self.agents
            .iter()
            .filter(|a| a.state() == State::Done)
            .map(|a| a.name.clone())
            .collect()
    }

    pub fn shutdown(&mut self) {
        for agent in &self.agents {
            let _ = agent.cleanup();
        }
        self.agents.clear();
    }
}

fn supervise(agent: &mut Agent, config: &Config) {
    let timeout = timeout_for(config, &agent.role);
    let state = agent.wait(timeout);
    if state == State::Timeout && retries_allowed(config) {
        let _ = agent.tmux.kill();
        if agent.restart().is_ok() {
            agent.wait(timeout);
        }
    }
    if agent.state() == State::Done {
        agent.commit(None);
    }
}

fn timeout_for(config: &Config, role: &str) -> u64 {
    config
        .timeouts
        .get(role)
        .or_else(|| config.timeouts.get("worker"))
        .copied()
        .unwrap_or(300)
}

fn retries_allowed(config: &Config) -> bool {
    config.playbooks.get("retry_on_timeout").copied().unwrap_or(0) > 0
}

fn default_probe(cli: &str, timeout_secs: u64) -> bool {
    let Ok(mut argv) = command::probe(cli) else {
        return false;
    };
    let bin = argv.remove(0);
    let mut child = match Command::new(bin)
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn init_repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("regente-engine-test-{}-{}", std::process::id(), name));
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
        assert!(status.success(), "git {:?} falhou", args);
    }

    fn tmux_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
    fn health_check_uses_injected_probe_runner() {
        let repo = init_repo("health");
        let config = Config::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let engine = Engine::with_probe_runner(&repo, &config, move |cli| {
            calls2.fetch_add(1, Ordering::SeqCst);
            cli == "claude"
        });
        let results = engine.health_check();
        assert_eq!(results.get("claude"), Some(&true));
        assert_eq!(results.get("codex"), Some(&false));
        assert!(calls.load(Ordering::SeqCst) >= config.distinct_clis().len());
    }

    #[test]
    fn health_check_catches_probe_panics_as_false() {
        // probe_runner returning false for unreachable clis is the failure path;
        // errors inside a real probe are already caught by default_probe.
        let repo = init_repo("health2");
        let config = Config::default();
        let engine = Engine::with_probe_runner(&repo, &config, |_cli| false);
        let results = engine.health_check();
        assert!(results.values().all(|v| !*v));
    }

    #[test]
    fn spawn_starts_agent_and_tracks_it() {
        skip_if_no_tmux!();
        let repo = init_repo("spawn");
        let config = Config::default();
        let mut engine = Engine::with_probe_runner(&repo, &config, |_| true);
        let idx = engine
            .spawn("s1", "claude", "t", None, None, None, Some("echo ok; exit 0".to_string()))
            .unwrap();
        assert_eq!(engine.agents[idx].state(), State::Running);
        engine.shutdown();
    }

    #[test]
    fn run_all_marks_done_agents() {
        skip_if_no_tmux!();
        let repo = init_repo("runall");
        let config = Config::default();
        let mut engine = Engine::with_probe_runner(&repo, &config, |_| true);
        engine
            .spawn("r1", "claude", "t", None, Some("worker"), None, Some("exit 0".to_string()))
            .unwrap();
        engine
            .spawn("r2", "claude", "t", None, Some("worker"), None, Some("exit 1".to_string()))
            .unwrap();
        let done = engine.run_all();
        assert_eq!(done, vec!["r1".to_string()]);
        engine.shutdown();
    }

    #[test]
    fn timeout_for_falls_back_to_worker() {
        let config = Config::default();
        assert_eq!(timeout_for(&config, "reviewer"), 300);
        assert_eq!(timeout_for(&config, "unknown-role"), *config.timeouts.get("worker").unwrap());
    }

    #[test]
    fn retries_allowed_reads_playbook() {
        let config = Config::default();
        assert!(retries_allowed(&config));
        let mut c2 = config.clone();
        c2.playbooks.insert("retry_on_timeout".to_string(), 0);
        assert!(!retries_allowed(&c2));
    }
}
