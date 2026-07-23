//! Layered configuration: defaults <- global (~/.config/rege/config.yml)
//! <- project (.rege.yml). Later layers deep-merge over earlier ones.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Master {
    pub cli: String,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RosterEntry {
    pub role: String,
    pub cli: String,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub master: Master,
    pub roster: Vec<RosterEntry>,
    pub timeouts: BTreeMap<String, u64>,
    pub playbooks: BTreeMap<String, u64>,
    pub pr: BTreeMap<String, String>,
    pub sandbox: BTreeMap<String, bool>,
    pub ui: BTreeMap<String, String>,
    #[serde(default)]
    pub verify: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        let roster = |role: &str, cli: &str, model: Option<&str>| RosterEntry {
            role: role.into(),
            cli: cli.into(),
            model: model.map(Into::into),
        };
        Config {
            // sonnet by default: cheap/fast orchestration; escalates to opus
            // via the planner/reviewer roles and the `consult` tool.
            master: Master { cli: "claude".into(), model: Some("sonnet".into()) },
            roster: vec![
                roster("triage", "claude", Some("haiku")),
                roster("planner", "claude", Some("opus")),
                roster("worker", "claude", Some("sonnet")),
                roster("worker", "codex", None),
                roster("worker", "opencode", None),
                roster("reviewer", "claude", Some("opus")),
                roster("bughunter", "claude", Some("fable")),
            ],
            timeouts: map(&[("triage", 60), ("worker", 300), ("reviewer", 300), ("healthcheck", 15)]),
            playbooks: map(&[("review_rounds", 3), ("retry_on_timeout", 1)]),
            pr: smap(&[("provider", "github"), ("branch_prefix", "rege")]),
            sandbox: bmap(&[("enabled", true), ("yolo", true)]),
            ui: smap(&[("theme", "hacker")]),
            verify: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn workers(&self) -> Vec<&RosterEntry> {
        self.roster.iter().filter(|r| r.role == "worker").collect()
    }

    pub fn distinct_clis(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for r in &self.roster {
            if !seen.contains(&r.cli) {
                seen.push(r.cli.clone());
            }
        }
        seen
    }

    pub fn global_path(home: &Path) -> PathBuf {
        home.join(".config/rege/config.yml")
    }

    /// defaults <- global <- project
    pub fn load(project_dir: Option<&Path>, home: &Path) -> Result<Config> {
        let mut merged = to_value(&Config::default())?;
        if let Some(v) = read_yaml(&Self::global_path(home))? {
            deep_merge(&mut merged, v);
        }
        if let Some(dir) = project_dir {
            if let Some(v) = read_yaml(&dir.join(".rege.yml"))? {
                deep_merge(&mut merged, v);
            }
        }
        Ok(serde_yaml::from_value(merged)?)
    }
}

fn read_yaml(path: &Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_yaml::from_str(&text)?))
}

fn to_value<T: Serialize>(v: &T) -> Result<Value> {
    Ok(serde_yaml::to_value(v)?)
}

/// Recursively merge `over` into `base` (maps merged key-wise, scalars replaced).
fn deep_merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Mapping(b), Value::Mapping(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) => deep_merge(existing, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

fn map(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}
fn smap(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}
fn bmap(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-cfg-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("home/.config/rege")).unwrap();
        fs::create_dir_all(d.join("proj")).unwrap();
        d
    }

    #[test]
    fn defaults_when_no_files() {
        let d = tmp("defaults");
        let c = Config::load(Some(&d.join("proj")), &d.join("home")).unwrap();
        assert_eq!(c.master.cli, "claude");
        assert_eq!(c.master.model.as_deref(), Some("sonnet"));
        assert!(!c.roster.is_empty());
        assert_eq!(c.ui.get("theme").unwrap(), "hacker");
    }

    #[test]
    fn global_overrides_defaults() {
        let d = tmp("global");
        fs::write(d.join("home/.config/rege/config.yml"), "master:\n  cli: gemini\n  model: pro\n").unwrap();
        let c = Config::load(Some(&d.join("proj")), &d.join("home")).unwrap();
        assert_eq!(c.master.cli, "gemini");
        assert_eq!(c.master.model.as_deref(), Some("pro"));
        // untouched keys keep defaults
        assert_eq!(c.ui.get("theme").unwrap(), "hacker");
    }

    #[test]
    fn project_overrides_global() {
        let d = tmp("project");
        fs::write(d.join("home/.config/rege/config.yml"), "master:\n  cli: gemini\n").unwrap();
        fs::write(d.join("proj/.rege.yml"), "master:\n  cli: codex\n").unwrap();
        let c = Config::load(Some(&d.join("proj")), &d.join("home")).unwrap();
        assert_eq!(c.master.cli, "codex");
    }

    #[test]
    fn deep_merge_preserves_siblings() {
        let d = tmp("deepmerge");
        fs::write(d.join("proj/.rege.yml"), "timeouts:\n  worker: 999\n").unwrap();
        let c = Config::load(Some(&d.join("proj")), &d.join("home")).unwrap();
        assert_eq!(*c.timeouts.get("worker").unwrap(), 999);
        assert_eq!(*c.timeouts.get("triage").unwrap(), 60);
    }
}
