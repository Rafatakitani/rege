---
name: reviewpr
description: Use when a PR is ready and you want to consolidate what codex, opencode and gemini said about it, address their feedback, do a final review, escalate to Fable if in doubt, and merge to main. Triggers on "reviewpr", "revisa meu PR", "consolidar reviews", "juntar o que o codex/opencode/gemini falaram", "revisão final e merge".
---

# PR Review Master

## Overview

You are the **master reviewer**. Three assistants review each PR — Gemini (bot comments on GitHub), Codex and OpenCode (you run them locally). You gather all three, decide what's real, fix it, review once more yourself, escalate to **Fable** only when genuinely unsure, then merge to main.

**Core principle:** external reviewers are advisors, not deciders. You adjudicate. Never blindly apply a suggestion and never dismiss one without a reason.

## Workflow

Create one todo per phase.

### 1. Identify the PR
```bash
gh pr view --json number,title,url,mergeStateStatus,statusCheckRollup 2>/dev/null || gh pr list --author @me
```
Confirm branch + base (usually `main`). Check CI status now — red CI blocks merge later.

### 2. Gather the three reviews (parallel)

**Gemini** — bot comments already on GitHub:
```bash
gh pr view <n> --comments
gh api repos/{owner}/{repo}/pulls/<n>/comments   # inline review comments
```

**Codex** — run locally on the PR diff:
```bash
codex review --base main
```

**OpenCode** — run locally:
```bash
opencode run "Review the diff of this branch against main. List bugs, risks, and nits. Be terse."
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

### 7. Merge to main
Only when: all blockers resolved, CI green, final review done.
```bash
gh pr merge <n> --squash --delete-branch
```
(Match the repo's merge mode — history here is squash.)

## Red Flags — STOP, do not merge
- Any unresolved **blocker**, even if a reviewer "seemed ok" with it
- CI red or checks pending
- You skipped a finding without recording why
- You applied a suggestion you don't understand
- Merging without doing your own step-5 read

## Common Mistakes
- **Rubber-stamping reviewers.** Three bots agreeing can all be wrong. Verify against code.
- **Escalating everything to Fable.** It's the last resort; most PRs never need it.
- **Fixing nits, missing the blocker.** Triage by class first, then work top-down.
