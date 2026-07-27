//! The interview: the master grills the user about the project and writes the
//! docs that come out of it.
//!
//! This is the counterpart to [`crate::scan`], not a variation of it. A scan
//! reads what the code already says — it needs no one present. An interview
//! reaches what the code cannot say: what is being built, what was decided and
//! why, what agents must not touch. That only exists in the user's head, so the
//! master has to ask, one question at a time.
//!
//! The master conducts it. rege hands over the script and the facts it already
//! collected for free, then stays out of the way — the conversation is already
//! there, and a fixed list of questions in Rust could not follow an answer
//! where it leads.
//!
//! The shape of the interview is borrowed from Matt Pocock's `grilling` and
//! `domain-modeling` agent skills: one question at a time, each carrying its own
//! recommendation, and decisions landing as ADRs plus a glossary at the end. The
//! script below is written from scratch, but the idea is theirs and worth
//! naming. (`docs/adr/` as a convention predates all of it — Michael Nygard,
//! 2011.)

use crate::scan::{self, Facts};

/// Directory holding the decision records the interview produces.
pub const ADR_DIR: &str = "docs/adr";

/// Where the domain vocabulary lands.
pub const GLOSSARY: &str = "docs/glossary.md";

/// The turn that starts the interview. Goes to the master as a request, never
/// shown in the chat — like the playbook, it is machinery, not conversation.
pub fn prompt(f: &Facts) -> String {
    let mut s = String::new();
    s.push_str(
        "Interview the user about this project, then write the documents that come \
         out of the interview. Do not delegate this to a worker and do not spawn \
         agents: this is a conversation between you and the user.\n\n",
    );

    s.push_str("HOW TO INTERVIEW\n");
    s.push_str(
        "- One question at a time. Wait for the answer before asking the next. \
         Several questions at once get one vague answer, or none.\n\
         - With every question, give your own recommended answer and say why. \
         The user is deciding, not filling in a form.\n\
         - Walk down the tree: let each answer decide what to ask next, and \
         resolve what a later decision depends on first.\n\
         - Never ask what the files already answer. Read the code and say what \
         you found instead: \"the Gemfile says Rails 8 — is Postgres the target?\"\n\
         - Push back when an answer is vague or contradicts an earlier one. \
         The point is a sharper plan, not a filled questionnaire.\n\
         - Stop when the answers stop changing what you would write. Say so and \
         move on to the documents.\n\n",
    );

    s.push_str("WHAT TO COVER (adapt to what you find; skip what the code answers)\n");
    s.push_str(
        "- What is being built, and for whom.\n\
         - What is already decided and must not be relitigated — stack, hosting, \
         data model — and the reason behind each, which is the part that decays \
         first.\n\
         - What agents must NOT do here: files to leave alone, dependencies not \
         to add, patterns already rejected.\n\
         - How to run and test it, if that is not obvious from the manifests.\n\
         - The domain vocabulary: the words this project uses for its own things, \
         and what each one means precisely.\n\n",
    );

    s.push_str("WHAT TO WRITE, AT THE END OF THE INTERVIEW\n");
    s.push_str(&format!(
        "- `{}`: what this project is, how to run and test it, conventions, and \
         what not to do. Short and specific. If the file already exists, merge \
         into it instead of flattening what is there.\n",
        scan::CONTEXT_FILE
    ));
    s.push_str(&format!(
        "- `{ADR_DIR}/NNN-<slug>.md`, one per decision that came up, numbered in \
         sequence from whatever is already in that directory. Each one: context, \
         the decision, the alternatives considered, the consequence. A decision \
         without its why is a decision nobody can revisit.\n"
    ));
    s.push_str(&format!(
        "- `{GLOSSARY}`: the domain terms and their meanings, only if the \
         interview produced vocabulary worth pinning down. An empty glossary is \
         worse than none.\n\n"
    ));
    s.push_str(
        "Write the files with your own tools, at the end. Then tell the user, in \
         two lines, which files you wrote and what is still open.\n\n",
    );

    s.push_str("WHAT rege ALREADY KNOWS ABOUT THIS DIRECTORY (do not ask again)\n");
    s.push_str(&scan::digest(f));
    if f.is_home {
        s.push_str(
            "\nWARNING: this directory is the user's HOME, not a project. Interview \
             them about how they work here, not about software architecture.\n",
        );
    }
    s.push_str("\nStart by asking your first question. Nothing else.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-grill-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn prompt_carries_the_script_and_the_facts() {
        let d = tmp("facts");
        fs::write(d.join("Gemfile"), "gem 'rails'\n").unwrap();
        let p = prompt(&scan::collect(&d, Path::new("/elsewhere")));

        // The script: one question at a time is the whole point of a grilling.
        assert!(p.contains("One question at a time"));
        assert!(p.contains("recommended answer"));
        // The facts, so the master doesn't open by asking what the tree shows.
        assert!(p.contains("Gemfile"), "the collected facts travel with it: {p}");
        assert!(p.contains("do not ask again"));
        // The documents it owes at the end.
        assert!(p.contains(scan::CONTEXT_FILE));
        assert!(p.contains(ADR_DIR));
        assert!(p.contains(GLOSSARY));
        // And it must not answer rege — it must ask the user.
        assert!(p.trim_end().ends_with("Nothing else."));
    }

    #[test]
    fn home_gets_warned_about_like_the_scan_does() {
        let d = tmp("home");
        let p = prompt(&scan::collect(&d, &d));
        assert!(p.contains("user's HOME"), "interviewing about a home is not interviewing about a project");
    }

    #[test]
    fn a_worker_never_gets_handed_the_interview() {
        let d = tmp("nodelegate");
        let p = prompt(&scan::collect(&d, Path::new("/elsewhere")));
        // The playbook tells the master to delegate everything; this is the one
        // job it must keep, since the user is in the room.
        assert!(p.contains("do not spawn"), "the master keeps the interview: {p}");
    }
}
