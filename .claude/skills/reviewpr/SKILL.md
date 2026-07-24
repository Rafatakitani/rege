---
name: reviewpr
description: Use when a PR is ready and you want to consolidate what codex and opencode (and optionally gemini) said about it, address their feedback, do a final review, escalate to Fable if in doubt, and hand it back for human approval. Triggers on "reviewpr", "revisa meu PR", "consolidar reviews", "juntar o que o codex/opencode falaram", "revisão final". Never merges on its own — AGENTS.md forbids it.
---

# PR Review Master

## Overview

You are the **master reviewer**. Two assistants always review each PR — Codex and OpenCode (you run them locally). A third, Gemini (bot comments on GitHub), is **optional**: use it only when the bot actually left comments; skip silently otherwise. You gather what's available, decide what's real, fix it, review once more yourself, escalate to **Fable** only when genuinely unsure, then **hand the PR back to the human for approval**.

> **This repo never merges by agent.** `AGENTS.md` is explicit: the final output is always a PR for human approval — never `gh pr merge`. This skill stops at "PR is clean and ready"; the human merges.

**Core principle:** external reviewers are advisors, not deciders. You adjudicate. Never blindly apply a suggestion and never dismiss one without a reason.

## Workflow

Create one todo per phase.

### 1. Identify the PR
```bash
gh pr view --json number,title,url,mergeStateStatus,statusCheckRollup 2>/dev/null || gh pr list --author @me
```
Confirm branch + base (usually `main`). Check CI status now — red CI blocks merge later.

### 2. Gather the reviews (parallel)

Run **after** CI's final check is green — no point reviewing a PR that doesn't build.

**Codex** (always) — run locally on the PR diff:
```bash
codex review --base main
```

**OpenCode** (always) — run locally:
```bash
opencode run "Review the diff of this branch against main. List bugs, risks, and nits. Be terse."
```

**Gemini** (optional) — only if a bot left comments; skip silently if none:
```bash
gh pr view <n> --comments
gh api repos/{owner}/{repo}/pulls/<n>/comments   # inline review comments
```

### 3. Triage — you decide

Merge all findings into one list. For each, classify:

| Class | Meaning | Action |
|-------|---------|--------|
| **Blocker** | Real bug, security, data loss, breaks build/tests | MUST fix before merge |
| **Should-fix** | Correctness edge case, missing test, clear smell | Fix unless justified |
| **Nit** | Style, naming, preference | Fix if cheap, else note & skip |
| **Wrong** | Reviewer misread the code | Skip, record one-line why |

Dedup — the three often flag the same thing. Agreement across reviewers ≠ automatically correct; verify against the actual code before acting.

### 4. Address
Fix blockers and should-fixes. Commit (no AI co-author trailer — see repo policy). Reply/resolve on GitHub where a comment was left.

### 5. Final review (you)
Read the *final* diff top to bottom yourself. Confirm every blocker is gone, tests/build green, scope matches the card. This is your review, not a rubber stamp of the assistants.

### 6. Escalate to Fable — only if unsure
When a decision is genuinely split (reviewers disagree, or you're not confident a fix is correct/complete), get a tie-breaker via a Fable subagent:

> Agent tool, `subagent_type: "claude"`, `model: "fable"` — give it the diff + the specific open question, ask for a verdict with reasoning.

Don't escalate routine PRs. Fable is the last resort, not a step.

### 7. Hand back for human approval — DO NOT merge
When all blockers are resolved, CI is green, and your final review is done, the PR is *ready* — stop here. Post a short summary on the PR (findings, what you fixed, what you skipped and why) and tell the human it's ready to merge.

**Never run `gh pr merge`.** Per `AGENTS.md`, merging is a human decision. The skill's job ends at a clean, review-complete PR.

## Red Flags — STOP
- Merging the PR yourself — the human does that, always
- Any unresolved **blocker**, even if a reviewer "seemed ok" with it
- CI red or checks pending
- You skipped a finding without recording why
- You applied a suggestion you don't understand
- Claiming "ready" without doing your own step-5 read

## Common Mistakes
- **Rubber-stamping reviewers.** Three bots agreeing can all be wrong. Verify against code.
- **Escalating everything to Fable.** It's the last resort; most PRs never need it.
- **Fixing nits, missing the blocker.** Triage by class first, then work top-down.
