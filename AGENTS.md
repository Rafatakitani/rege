# AGENTS.md

Instructions for AIs / agents operating this repo or using `rege` as a tool.
(The `AGENTS.md` convention — read by Codex, Claude Code and friends.)

## What `rege` is

A multi-agent orchestrator: a **master** (the main model) commands other AI CLIs as
**workers**, each isolated in a `git worktree` + `tmux` session. The master triages the
difficulty, delegates, reviews and **opens a PR** — it never merges on its own.

## How you (an AI) invoke `rege`

**Headless, one task (like `codex exec`):**
```bash
rege exec "describe the task here"
```
That already runs the master with the playbook + MCP server: it can spawn workers, wait
on them, review and open a PR by itself. Use it to delegate a whole task.

**Interactive, in master mode:**
```bash
rege claude   # opens claude already as the orchestrator (playbook + MCP + auto-approval)
```

**The MCP server alone (to plug into your own MCP client):**
```bash
rege mcp-serve --repo /path/to/repo
```
It speaks newline-delimited JSON-RPC 2.0 over stdio: `initialize`, `tools/list`,
`tools/call`.

**Inspecting the TUI without a terminal (useful in CI/headless):**
```bash
rege render --demo
```

## Available MCP tools

When you are the **master**, you have these tools (over MCP):

| tool | what it does |
|------|--------------|
| `spawn_agent` | dispatches a worker (AI CLI) isolated in a git worktree for a task |
| `list_agents` | lists the agents and their states |
| `agent_status` | current state of one agent |
| `wait_agent` | **blocks until the agent finishes** (or timeout) and commits the work — use after `spawn_agent` |
| `read_output` | accumulated output of one agent |
| `send_message` | injects text into an agent's session (redirect / whisper / take over) |
| `kill_agent` | kills an agent's session |
| `diff_agent` | diff of an agent's branch |
| `review` | builds the review context: diffs of the given agents' branches |
| `run_tests` | runs the verify command in the agent's worktree (if configured) |
| `consult` | one-off question to a stronger model (e.g. opus) without spawning a worker — reasoning escalation |
| `open_pr` | opens a PR from a branch (**never merges**); fallback: a local patch |

## Rules (not negotiable)

1. **Workers are isolated** in a `git worktree` — the current branch is never touched
   directly.
2. **Always wait** for the worker (`wait_agent`) before reviewing; otherwise `-p` exits
   first.
3. **Never merge on your own.** The final output is always a **PR** (`open_pr`) for human
   approval — sometimes it needs review from other people too.
4. **Triage:** easy = divide & conquer (workers on different parts → merge → review).
   Hard = redundancy & judge (several do the same thing → synthetic merge → repair loop,
   3 rounds max, run the tests if any exist).
5. **Escalate sparingly:** cheap by default (sonnet, say); use `consult`/opus only when
   the decision is hard.
6. If something hangs, **kill it and move on** with whoever finished (per-worker
   timeout).

## Working IN THIS repository (rege itself)

- Rust. `cargo test` (192 tests) must pass before committing. `cargo fmt` +
  `cargo clippy`.
- **The whole product speaks English** — TUI chrome, overlays, errors, `/help`, clap
  help, MCP tool descriptions, the prompts sent to the master, code comments and these
  docs. The playbook tells the master to answer in whatever language the user writes in,
  so a Portuguese-speaking user still gets Portuguese back. Don't mix the two in the
  source again.
- **`Cargo.lock` is committed** and `rege update` installs with `--locked`. Without it
  `cargo install --git` re-resolves all 94 dependencies and refreshes the crates.io index
  on every update, installing versions nobody tested. Commit the lock together with the
  version bump.
- **`update` only compiles when there is a new commit.** A `git ls-remote` answers in ~1s
  whether the remote is already on the commit stamped into the binary; if it is, the
  update ends there. `--force` skips the check. When the probe fails **because of the
  network**, the update stops right away with a message — cargo's own fetch would fail
  too, only a minute later. Any other failure (credentials, missing ref) falls through to
  the build, which is what has the verdict.
- **`update` compiles with the `fastinstall` profile** (opt-level 1), not `release`. What
  is optimized there is build time; the binary is IO-bound. If the commit is too old for
  the flags, `update` retries without them instead of breaking — but that costs two
  builds, so don't drop the profile or the lock.
- **Version**: `version` in `Cargo.toml` goes up in every PR that changes behaviour —
  otherwise `rege --version` can't tell builds apart and nobody knows whether the update
  landed. `build.rs` stamps the commit on top (`rege 0.2.0 (b178154)`); the hash is
  automatic, the semver is manual.
- **TUI overlays clear with `Clear`**, never with `Paragraph::new("")` — an empty
  Paragraph draws nothing and leaves whatever is underneath showing through the panel.
  This was a bug in all five overlays at once already.
- **Palettes carry four levels**, not one hue: `strong` (headings, bold) > `text` (prose)
  > `dim` (chrome), plus `accent` for the theme's marks and `accent2` for code, on a
  deliberately different hue. A test enforces the separation — `luxury` used to paint
  everything gold and the whole conversation read as one flat wash.
- Modules in `src/`: `config`, `command`, `worktree`, `tmux`, `agent`, `engine`,
  `session`, `mcp`, `theme`, `tui`, `buddy`, `stream`, `driver`, `sessions`, `playbook`,
  `rtk`, `scan`, `grill`.
- **`rtk`**: if the [`rtk`](https://github.com/rtk-ai/rtk) binary is on the `PATH`, the
  diff that goes into the master's context (`diff_agent`/`review`) passes through
  `rtk git diff` (-75% tokens). `open_pr`'s `.patch` and the git plumbing stay raw — a
  condensed diff doesn't apply. Rule when touching this: **only compress what an LLM will
  read**; whatever a machine consumes stays raw.
  Precedence lives in one place (`rtk::resolve`): `REGE_RTK` > `config.yml` >
  autodetection on the `PATH`. Don't add a second knob for the same decision.
  `rtk.hook_workers` (explicit opt-in) runs `rtk init --hook-only` inside the worktree of
  every worker listed in `rtk.clis` — autodetection never turns that on by itself.
- **`transcript`**: `/resume` repaints the past conversation by reading the JSONL that
  `claude` itself keeps in `~/.claude/projects/<slug>/<id>.jsonl`. It is best-effort and
  only works for `claude` — another CLI (or cleared history) resumes without the replay,
  as before. We don't invent a transcript format: if the source doesn't exist, there is
  nothing to show. The user's request sits after a marker that was `Tarefa:` and is now
  `Task:`; the strip accepts both, or `/resume` on an old session would dump the whole
  playbook on screen.
- **`scan`**: reads what the code already says. Deterministic, capped collection
  (`MAX_FILES`/`MAX_DEPTH`) + one call to the master with the digest ready. The answer
  lives in `~/.config/rege/scanned.yml`, never in the user's project; it never overwrites
  an existing `AGENTS.md` without `--force`.
- **`grill`**: the counterpart to `scan`. A scan reads what the code says; the interview
  reaches what the code cannot say (what is being built, why decisions were made, what
  not to touch), which is why the **master** conducts it — one question at a time, in the
  conversation that already exists. rege only hands over the script and the facts it
  already collected. The answer to the overlay is recorded as `grill` (distinct from
  `yes`) so it stays knowable which route a directory took. Don't inject the script into
  the chat: what the user must see is the first question, not the briefing.
- Full design: `docs/superpowers/specs/2026-07-23-rege-design.md`.
- Keep `/buddy`'s MIT attribution (a port of ramarivera/claude-buddy) at the top of
  `src/buddy.rs`, and the credit to Matt Pocock's `grilling`/`domain-modeling` skills at
  the top of `src/grill.rs` — the prompt is ours, the shape of the interview is not.
