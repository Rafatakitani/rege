//! One isolated git worktree + branch per agent. Agents edit here in parallel
//! with zero file-level races; the reviewer diffs each branch.

use crate::rtk;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Worktree {
    pub repo: PathBuf,
    pub name: String,
    pub branch: String,
    pub path: PathBuf,
    base: Option<String>,
}

impl Worktree {
    pub fn new(
        repo: &Path,
        name: &str,
        branch_prefix: Option<&str>,
        base: Option<&str>,
        root: Option<&Path>,
    ) -> Result<Self> {
        let repo = std::fs::canonicalize(repo)?;
        let branch_prefix = branch_prefix.unwrap_or("rege");
        let branch = format!("{}/{}", branch_prefix, name);
        let root = match root {
            Some(r) => r.to_path_buf(),
            None => {
                let repo_name = repo
                    .file_name()
                    .ok_or_else(|| anyhow!("repo has no directory name"))?;
                std::env::temp_dir().join("rege-worktrees").join(repo_name)
            }
        };
        let path = root.join(name);
        Ok(Worktree {
            repo,
            name: name.to_string(),
            branch,
            path,
            base: base.map(str::to_string),
        })
    }

    pub fn create(&self) -> Result<PathBuf> {
        let root = self.path.parent().ok_or_else(|| anyhow!("path sem parent"))?;
        std::fs::create_dir_all(root)?;
        // Clear metadata left by a worktree whose directory was deleted out from
        // under git (e.g. a crashed run under /tmp) — otherwise `worktree add`
        // aborts with "branch already used by worktree at <gone path>".
        let _ = self.git(&["worktree", "prune"]);
        let base = match &self.base {
            Some(b) => b.clone(),
            None => self.current_head()?,
        };
        self.git(&[
            "worktree",
            "add",
            "-b",
            &self.branch,
            self.path.to_str().ok_or_else(|| anyhow!("path invalido"))?,
            &base,
        ])?;
        Ok(self.path.clone())
    }

    pub fn commit_all(&self, message: &str) -> Result<()> {
        self.git_in(&self.path, &["add", "-A"])?;
        self.git_in(&self.path, &["commit", "-q", "-m", message])?;
        Ok(())
    }

    /// Diff of this branch against the base ref it forked from.
    ///
    /// This one feeds the master's context (`diff_agent`, `review`), so it goes
    /// through `rtk` when available — condensed, ~75% fewer tokens. The raw diff
    /// used to write `.patch` files lives in `session::write_patch` and must stay
    /// raw to remain appliable.
    pub fn diff(&self) -> Result<String> {
        let base = self.base.clone().unwrap_or_else(|| "HEAD".to_string());
        let range = format!("{}...{}", base, self.branch);
        let repo = self.repo.to_str().ok_or_else(|| anyhow!("path invalido"))?;
        let argv = rtk::git_argv(&["-C", repo, "diff", &range]);
        let output = Command::new(&argv[0]).args(&argv[1..]).output()?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn remove(&self, force: bool) -> Result<()> {
        let mut args = vec!["worktree".to_string(), "remove".to_string()];
        if force {
            args.push("--force".to_string());
        }
        args.push(self.path.to_str().ok_or_else(|| anyhow!("path invalido"))?.to_string());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let _ = self.git_capture(&arg_refs); // tolerate already-gone
        let _ = self.git_capture(&["branch", "-D", &self.branch]); // best-effort branch cleanup
        if self.path.exists() {
            std::fs::remove_dir_all(&self.path)?;
        }
        Ok(())
    }

    pub fn exists(&self) -> bool {
        self.path.is_dir()
    }

    pub fn list(repo: &Path) -> Result<Vec<String>> {
        let repo = std::fs::canonicalize(repo)?;
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "list", "--porcelain"])
            .output()?;
        let out = String::from_utf8_lossy(&output.stdout);
        Ok(out
            .lines()
            .filter_map(|l| l.strip_prefix("worktree ").map(str::to_string))
            .collect())
    }

    fn current_head(&self) -> Result<String> {
        let (out, _) = self.git_capture(&["rev-parse", "HEAD"])?;
        Ok(out.trim().to_string())
    }

    fn git(&self, args: &[&str]) -> Result<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("git {} failed: {}", args.join(" "), err));
        }
        Ok(())
    }

    fn git_capture(&self, args: &[&str]) -> Result<(String, String)> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()?;
        Ok((
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }

    fn git_in(&self, dir: &Path, args: &[&str]) -> Result<()> {
        let output = Command::new("git").arg("-C").arg(dir).args(args).output()?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("git {} failed: {}", args.join(" "), err));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init_repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-wt-{}-{}", std::process::id(), name));
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

    #[test]
    fn create_makes_worktree_and_branch() {
        let repo = init_repo("create");
        let wt = Worktree::new(&repo, "agent1", None, None, None).unwrap();
        let path = wt.create().unwrap();
        assert!(path.is_dir());
        assert_eq!(wt.branch, "rege/agent1");
        assert!(wt.exists());
        wt.remove(true).unwrap();
    }

    #[test]
    fn commit_all_and_diff() {
        let repo = init_repo("commit");
        let wt = Worktree::new(&repo, "agent2", None, None, None).unwrap();
        wt.create().unwrap();
        fs::write(wt.path.join("new.txt"), "content\n").unwrap();
        wt.commit_all("add new file").unwrap();
        let diff = wt.diff().unwrap();
        assert!(diff.contains("new.txt"));
        assert!(diff.contains("content"));
        wt.remove(true).unwrap();
    }

    #[test]
    fn remove_cleans_up() {
        let repo = init_repo("remove");
        let wt = Worktree::new(&repo, "agent3", None, None, None).unwrap();
        wt.create().unwrap();
        assert!(wt.exists());
        wt.remove(true).unwrap();
        assert!(!wt.exists());
    }

    #[test]
    fn list_includes_created_worktree() {
        let repo = init_repo("list");
        let wt = Worktree::new(&repo, "agent4", None, None, None).unwrap();
        wt.create().unwrap();
        let list = Worktree::list(&repo).unwrap();
        assert!(list.iter().any(|p| p.contains("agent4")));
        wt.remove(true).unwrap();
    }

    #[test]
    fn custom_branch_prefix_and_root() {
        let repo = init_repo("custom");
        let root = std::env::temp_dir().join(format!("rege-wt-custom-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let wt = Worktree::new(&repo, "agent5", Some("feat"), None, Some(&root)).unwrap();
        wt.create().unwrap();
        assert_eq!(wt.branch, "feat/agent5");
        assert_eq!(wt.path, root.join("agent5"));
        wt.remove(true).unwrap();
    }
}
