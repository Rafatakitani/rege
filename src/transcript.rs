//! Reads back the master's past conversation from the driver CLI's own
//! transcript, so `/resume` can show what was said instead of an empty screen
//! with "resuming session: <title>".
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

/// Beyond this the slug is cut and a hash of the full path is appended, so two
/// long paths sharing a prefix still land in different directories. Claude's
/// own limit; ours has to be the same number or the path misses.
const MAX_SLUG: usize = 200;

/// Claude's directory naming: **every** character outside `[a-zA-Z0-9]` becomes
/// `-`, so `/home/u/proj/.claude` becomes `-home-u-proj--claude` and
/// `/home/u/invasão-alien` becomes `-home-u-invas-o-alien`. Flattening only `/`
/// and `.` was enough for ASCII paths and silently missed every accented one —
/// the file was there, we were looking one directory over.
///
/// It runs over UTF-16 code units (a JS `replace` without `/u`), so an astral
/// character like an emoji turns into two dashes, not one.
pub fn project_slug(repo: &str) -> String {
    let slug: String = repo
        .encode_utf16()
        .map(|u| match u8::try_from(u) {
            Ok(b) if b.is_ascii_alphanumeric() => b as char,
            _ => '-',
        })
        .collect();
    if slug.len() <= MAX_SLUG {
        return slug;
    }
    // Every char is ASCII by now, so slicing by byte is slicing by char.
    format!("{}-{}", &slug[..MAX_SLUG], slug_hash(repo))
}

/// The `(h << 5) - h + c` string hash, kept in wrapping i32 like the JS `| 0`,
/// then `Math.abs(...).toString(36)`. `abs` of `i32::MIN` is what pushes this
/// through `unsigned_abs`: in JS it's `2147483648`, not an overflow.
fn slug_hash(repo: &str) -> String {
    let mut h: i32 = 0;
    for u in repo.encode_utf16() {
        h = h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(u));
    }
    let mut n = u64::from(h.unsigned_abs());
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(char::from_digit((n % 36) as u32, 36).unwrap());
        n /= 36;
    }
    out.iter().rev().collect()
}

pub fn transcript_path(home: &Path, repo: &str, session_id: &str) -> PathBuf {
    home.join(".claude/projects").join(project_slug(repo)).join(format!("{session_id}.jsonl"))
}

/// The first turn carries the whole playbook ahead of the real request, since
/// that's how the master is seeded. Show only what the user actually typed.
fn strip_playbook(text: &str) -> &str {
    // `Tarefa:` is the marker rege used before the UI moved to English.
    // Transcripts already on disk still carry it, and `/resume` has to keep
    // reading them — dropping it would replay the whole playbook on screen.
    for marker in ["\n\nTask: ", "\n\nTarefa: "] {
        if let Some(i) = text.rfind(marker) {
            return &text[i + marker.len()..];
        }
    }
    text
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
    fn slug_flattens_accents_like_claude_does() {
        // Checked against the directory `claude` actually created for this repo:
        // `~/.claude/projects/-home-rafa-projetos-invas-o-alien-3d`.
        assert_eq!(
            project_slug("/home/rafa/projetos/invasão-alien-3d"),
            "-home-rafa-projetos-invas-o-alien-3d"
        );
        // Spaces, underscores, everything outside [a-zA-Z0-9].
        assert_eq!(project_slug("/home/u/my proj_v2"), "-home-u-my-proj-v2");
        // Astral characters are two UTF-16 units, so two dashes.
        assert_eq!(project_slug("/a/🙂"), "-a---");
    }

    #[test]
    fn a_very_long_path_is_cut_and_hashed() {
        let repo = format!("/home/u/{}", "x".repeat(300));
        let slug = project_slug(&repo);
        let (head, hash) = slug.rsplit_once('-').unwrap();
        assert_eq!(head.len(), MAX_SLUG, "cut at claude's limit: {slug}");
        assert!(hash.chars().all(|c| c.is_ascii_alphanumeric()), "base36 tail: {hash}");
        // Same prefix, different path — the hash is what keeps them apart.
        assert_ne!(slug, project_slug(&format!("{repo}y")));
        // Checked against claude's own `Math.abs(hash).toString(36)`.
        assert_eq!(hash, "giky6d");
        assert!(project_slug(&format!("{repo}y")).ends_with("-es91k2"));
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

