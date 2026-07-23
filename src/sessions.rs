//! Persisted history of master-driver sessions, so `/resume` can offer past
//! conversations to continue via `--resume <session_id>` (see `driver.rs`).
//! Stored as a flat JSON array at `~/.local/share/regente/sessions.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECORDS: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRec {
    pub id: String,
    pub title: String,
    pub repo: String,
    pub ts: u64,
}

pub fn now_ts() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn data_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn default_path() -> PathBuf {
    data_home().join(".local/share/regente/sessions.json")
}

pub fn load(path: &Path) -> Vec<SessionRec> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Prepend `rec`, removing any existing record with the same id, then cap
/// the list at `MAX_RECORDS` (dropping the oldest / tail entries).
pub fn add(path: &Path, rec: SessionRec) {
    let mut records = load(path);
    records.retain(|r| r.id != rec.id);
    records.insert(0, rec);
    records.truncate(MAX_RECORDS);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&records) {
        let _ = std::fs::write(path, text);
    }
}

pub fn recent(path: &Path, n: usize) -> Vec<SessionRec> {
    load(path).into_iter().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("regente-sessions-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        d.join("sessions.json")
    }

    fn rec(id: &str, ts: u64) -> SessionRec {
        SessionRec { id: id.into(), title: format!("titulo {id}"), repo: "/repo".into(), ts }
    }

    #[test]
    fn add_creates_file_and_load_recovers() {
        let path = tmp("basic");
        assert!(load(&path).is_empty());
        add(&path, rec("a", 1));
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "a");
    }

    #[test]
    fn add_dedups_by_id_and_moves_to_front() {
        let path = tmp("dedup");
        add(&path, rec("a", 1));
        add(&path, rec("b", 2));
        add(&path, SessionRec { id: "a".into(), title: "atualizado".into(), repo: "/repo".into(), ts: 3 });
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "a");
        assert_eq!(loaded[0].title, "atualizado");
        assert_eq!(loaded[1].id, "b");
    }

    #[test]
    fn add_caps_at_50_most_recent() {
        let path = tmp("cap");
        for i in 0..60u64 {
            add(&path, rec(&format!("s{i}"), i));
        }
        let loaded = load(&path);
        assert_eq!(loaded.len(), 50);
        assert_eq!(loaded[0].id, "s59");
        assert_eq!(loaded[49].id, "s10");
    }

    #[test]
    fn recent_limits_count() {
        let path = tmp("recent");
        for i in 0..5u64 {
            add(&path, rec(&format!("s{i}"), i));
        }
        let r = recent(&path, 3);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].id, "s4");
    }
}
