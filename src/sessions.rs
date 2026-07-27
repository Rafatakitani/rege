//! Persisted history of master-driver sessions, so `/resume` can offer past
//! conversations to continue via `--resume <session_id>` (see `driver.rs`).
//! Stored as a flat JSON array at `~/.local/share/rege/sessions.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The file is shared by every repo and `/resume` only shows the current one's
/// sessions, so the cap has to hold several repos' history at once.
const MAX_RECORDS: usize = 200;

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
    data_home().join(".local/share/rege/sessions.json")
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

/// Most recent sessions started in `repo`. The file is shared by every repo,
/// and a session only resumes inside the directory it was started in, so
/// offering another repo's sessions was offering broken resumes.
pub fn recent_for(path: &Path, repo: &str, n: usize) -> Vec<SessionRec> {
    load(path).into_iter().filter(|r| r.repo == repo).take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-sessions-{}-{}", std::process::id(), name));
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
    fn add_caps_at_max_records_most_recent() {
        let path = tmp("cap");
        let total = MAX_RECORDS as u64 + 10;
        for i in 0..total {
            add(&path, rec(&format!("s{i}"), i));
        }
        let loaded = load(&path);
        assert_eq!(loaded.len(), MAX_RECORDS);
        assert_eq!(loaded[0].id, format!("s{}", total - 1));
        assert_eq!(loaded[MAX_RECORDS - 1].id, "s10");
    }

    #[test]
    fn recent_limits_count() {
        let path = tmp("recent");
        for i in 0..5u64 {
            add(&path, rec(&format!("s{i}"), i));
        }
        let r = recent_for(&path, "/repo", 3);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].id, "s4");
    }

    /// One shared file, many repos: `/resume` in one repo must not offer
    /// sessions started in another — resuming them there doesn't work.
    #[test]
    fn recent_only_returns_sessions_from_the_asked_repo() {
        let path = tmp("by-repo");
        add(&path, rec("a", 1));
        add(&path, SessionRec { id: "b".into(), title: "outro".into(), repo: "/other".into(), ts: 2 });
        add(&path, rec("c", 3));
        let mine: Vec<_> = recent_for(&path, "/repo", 12).into_iter().map(|r| r.id).collect();
        assert_eq!(mine, ["c", "a"]);
        let theirs: Vec<_> = recent_for(&path, "/other", 12).into_iter().map(|r| r.id).collect();
        assert_eq!(theirs, ["b"]);
        assert!(recent_for(&path, "/nowhere", 12).is_empty());
    }
}
