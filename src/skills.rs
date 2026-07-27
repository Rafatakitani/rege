//! Discovery of the user's own Claude Code skills and slash commands, so they
//! can be typed inside rege.
//!
//! rege has its own commands (`/theme`, `/scan`, …) and the master has the
//! user's (`/explain`, `/reviewpr`, …). Typing one of the latter used to land
//! on "unknown command", which is a lie: the master is `claude`, and `claude -p
//! "/explain x"` runs the skill perfectly well. All rege has to do is know the
//! names, so it can forward what belongs to the master and still fail cheaply
//! on a typo — forwarding blindly would turn `/quti` into a paid turn.
//!
//! Names only. What a skill *does* is the master's business; rege never reads
//! the bodies.

use std::collections::BTreeSet;
use std::path::Path;

/// Where Claude Code keeps them, relative to a root (`~` or the project).
const SKILL_DIRS: [&str; 2] = [".claude/skills", ".claude/commands"];

/// Every skill/command name available from `home` and `repo`, sorted and
/// deduplicated — a project skill and a user skill of the same name are one
/// command, and the master resolves which wins.
pub fn discover(home: &Path, repo: &Path) -> Vec<String> {
    let mut found = BTreeSet::new();
    for root in [home, repo] {
        for dir in SKILL_DIRS {
            collect_into(&root.join(dir), &mut found);
        }
    }
    found.into_iter().collect()
}

/// A skill is a directory holding a `SKILL.md`; a command is a bare `.md`.
/// Anything else in there (README, a stray file) is not a command and must not
/// show up in the menu.
fn collect_into(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) if !n.starts_with('.') => n.to_string(),
            _ => continue,
        };
        if path.is_dir() {
            if path.join("SKILL.md").is_file() {
                out.insert(name);
            }
        } else if path.extension().is_some_and(|e| e == "md") {
            out.insert(name);
        }
    }
}

/// Is `line` a slash command belonging to the master rather than to rege?
/// Matches on the command word alone, so arguments come along for the ride.
pub fn matches(names: &[String], line: &str) -> bool {
    let Some(word) = line.strip_prefix('/').map(|l| l.split_whitespace().next().unwrap_or("")) else {
        return false;
    };
    !word.is_empty() && names.iter().any(|n| n == word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-skills-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn skill(root: &Path, name: &str) {
        let d = root.join(".claude/skills").join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), "---\nname: x\n---\n").unwrap();
    }

    #[test]
    fn finds_skills_and_commands_from_home_and_project() {
        let home = tmp("home");
        let repo = tmp("repo");
        skill(&home, "explain");
        fs::create_dir_all(home.join(".claude/commands")).unwrap();
        fs::write(home.join(".claude/commands/deploy.md"), "do it\n").unwrap();
        skill(&repo, "reviewpr");

        let names = discover(&home, &repo);
        assert!(names.contains(&"explain".to_string()), "user skill: {names:?}");
        assert!(names.contains(&"deploy".to_string()), "user command: {names:?}");
        assert!(names.contains(&"reviewpr".to_string()), "project skill: {names:?}");
    }

    #[test]
    fn the_same_name_in_both_places_is_one_command() {
        let home = tmp("dup-home");
        let repo = tmp("dup-repo");
        skill(&home, "explain");
        skill(&repo, "explain");
        assert_eq!(discover(&home, &repo).iter().filter(|n| *n == "explain").count(), 1);
    }

    #[test]
    fn a_directory_without_a_skill_file_is_not_a_command() {
        let home = tmp("empty");
        fs::create_dir_all(home.join(".claude/skills/half-written")).unwrap();
        // A README next to the skills is documentation, not a command.
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let names = discover(&home, Path::new("/nonexistent"));
        assert!(names.is_empty(), "nothing here is runnable: {names:?}");
    }

    #[test]
    fn missing_directories_are_normal_not_an_error() {
        assert!(discover(Path::new("/nonexistent"), Path::new("/also-not-here")).is_empty());
    }

    #[test]
    fn matching_takes_the_command_word_and_leaves_the_arguments() {
        let names = vec!["explain".to_string(), "reviewpr".to_string()];
        assert!(matches(&names, "/explain"));
        assert!(matches(&names, "/explain what a worktree is"), "arguments ride along");
        // A typo stays a typo: cheap error instead of a paid turn.
        assert!(!matches(&names, "/explainn"));
        assert!(!matches(&names, "/quti"));
        assert!(!matches(&names, "explain"), "no slash, no command");
        assert!(!matches(&names, "/"));
    }
}
