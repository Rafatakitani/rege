//! Reads back the master's past conversation from the driver CLI's own
//! transcript, so `/resume` can show what was said instead of an empty screen
//! with "retomando sessão: <título>".
//!
//! Only `claude` is supported: it keeps a JSONL transcript per session under
//! `~/.claude/projects/<slug>/<session-id>.jsonl`, and this reads that file.
//! Other CLIs get an empty list rather than a guess — the resume itself still
//! works, it just starts visually blank as before.

use std::path::{Path, PathBuf};

/// A replayed turn: who spoke and what was said, already stripped of the
/// machinery (tool calls, tool results, thinking) that the chat pane doesn't
/// show for live turns either.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub from_user: bool,
    pub text: String,
}

/// Claude's directory naming: the absolute path with `/` and `.` flattened to
/// `-`, so `/home/u/proj/.claude` becomes `-home-u-proj--claude`.
pub fn project_slug(repo: &str) -> String {
    repo.chars().map(|c| if c == '/' || c == '.' { '-' } else { c }).collect()
}

pub fn transcript_path(home: &Path, repo: &str, session_id: &str) -> PathBuf {
    home.join(".claude/projects").join(project_slug(repo)).join(format!("{session_id}.jsonl"))
}

/// The first turn carries the whole playbook ahead of the real request, since
/// that's how the master is seeded. Show only what the user actually typed.
fn strip_playbook(text: &str) -> &str {
    match text.rfind("\n\nTarefa: ") {
        Some(i) => &text[i + "\n\nTarefa: ".len()..],
        None => text,
    }
}

/// Extracts the visible conversation. Anything unparseable is skipped rather
/// than failing the whole read — a truncated last line is normal in a JSONL
/// file that a live process appends to.
pub fn read(path: &Path) -> Vec<Turn> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut turns = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let from_user = match kind {
            "user" => true,
            "assistant" => false,
            _ => continue,
        };
        let Some(content) = v.get("message").and_then(|m| m.get("content")) else { continue };
        let mut said = String::new();
        match content {
            // A plain string is the user typing; the first one has the playbook
            // glued in front of it.
            serde_json::Value::String(s) => said.push_str(strip_playbook(s).trim()),
            serde_json::Value::Array(blocks) => {
                for b in blocks {
                    // `text` is speech; tool_use/tool_result/thinking are
                    // machinery and stay out, matching the live chat pane.
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            if !said.is_empty() {
                                said.push('\n');
                            }
                            said.push_str(t.trim());
                        }
                    }
                }
            }
            _ => continue,
        }
        if !said.is_empty() {
            turns.push(Turn { from_user, text: said });
        }
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-transcript-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn slug_flattens_slashes_and_dots() {
        assert_eq!(project_slug("/home/u/proj"), "-home-u-proj");
        // `.claude` becomes `-claude`, which is why the double dash shows up.
        assert_eq!(project_slug("/home/u/proj/.claude/skills"), "-home-u-proj--claude-skills");
    }

    #[test]
    fn path_lands_where_claude_keeps_the_session() {
        let p = transcript_path(Path::new("/home/u"), "/home/u/proj", "abc-123");
        assert_eq!(p, PathBuf::from("/home/u/.claude/projects/-home-u-proj/abc-123.jsonl"));
    }

    #[test]
    fn read_keeps_speech_and_drops_the_machinery() {
        let d = tmp("read");
        let f = d.join("s.jsonl");
        let lines = [
            // First user turn: playbook glued in front of the real request.
            r#"{"type":"user","message":{"role":"user","content":"Voce e o MESTRE...\n\nTarefa: me faca um resumo"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"spawn_agent"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"É um app Rails."}]}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","content":"ignorar"}"#,
            r#"{"type":"last-prompt","lastPrompt":"ignorar"}"#,
            // A truncated tail is normal while the CLI is still appending.
            r#"{"type":"assistant","message":{"role":"assis"#,
        ];
        fs::write(&f, lines.join("\n")).unwrap();

        let turns = read(&f);
        assert_eq!(turns.len(), 2, "só fala entra: {turns:?}");
        assert_eq!(turns[0], Turn { from_user: true, text: "me faca um resumo".into() }, "playbook fica fora");
        assert_eq!(turns[1], Turn { from_user: false, text: "É um app Rails.".into() });
    }

    #[test]
    fn read_of_a_missing_file_is_empty_not_an_error() {
        assert!(read(Path::new("/nao/existe/x.jsonl")).is_empty());
    }

    #[test]
    fn later_turns_have_no_playbook_to_strip() {
        let d = tmp("later");
        let f = d.join("s.jsonl");
        fs::write(&f, r#"{"type":"user","message":{"role":"user","content":"e agora?"}}"#).unwrap();
        assert_eq!(read(&f)[0].text, "e agora?");
    }
}
