# Rege

> ⚠️ Working title · project under development.

Multi-agent AI orchestrator, in Rust. A terminal TUI where you talk to a **master**
(the main model, `claude` by default — swappable) that **commands other AI CLIs**
(`claude`, `codex`, `gemini`, `opencode`) as workers. The master sizes the task up,
puts a team together, lets each worker run **isolated in a `git worktree` + `tmux`
session**, reviews the result and **opens a PR** — it never merges on its own.

```
you ⇄ master's TUI  →  master (claude/…) via MCP  →  Rege
                                                       │
                     ┌──────────────┬─────────────┬────┘
               worker (worktree A)  (worktree B)  (worktree C)  → review → PR
```

In a single process, the binary is: an **MCP server** (exposing the tools to the
master), a **tmux/worktree controller** (spawning, watching and injecting into
workers), and the **TUI**.

## ⚠️ Important warning

Workers run **auto-approving** (`--dangerously-skip-permissions` / `--yolo` / codex's
sandbox): they edit files, run commands and commit **without asking**. They are confined
to a `git worktree` — your current branch is never touched directly, and the final output
is always a **PR for human approval**. Even so: **only run this on repos you control, and
understand what you are asking for.** It is not a security sandbox.

## Requirements

- Rust / Cargo (the installer below sets this up for you if it's missing)
- `git`, `tmux`
- At least one AI CLI installed and authenticated. **Today the master is only fully
  wired for `claude`**; `codex`/`gemini`/`opencode` work as workers on a best-effort
  basis.
- `gh` (optional), authenticated, to open PRs — without it, it falls back to a `.patch`.
- [`rtk`](https://github.com/rtk-ai/rtk) (optional) — compresses the output that enters
  the master's context. See [Token thrift](#token-thrift-rtk).

## Installation

One command — even on a machine without Rust:

```bash
curl -fsSL https://raw.githubusercontent.com/Rafatakitani/rege/main/install.sh | sh
```

It checks for `git`, installs Rust via [rustup](https://rustup.rs) when `cargo` is
missing (user-space, no sudo), compiles the latest rege straight from this repo
(same profile and build cache `rege update` uses, so later updates start warm),
and tells you if `tmux` still needs installing. `REGE_BRANCH=x` in front pins a
branch or tag. As with any `curl | sh`, feel free to
[read the script](install.sh) first.

Prefer to do it by hand?

```bash
git clone https://github.com/Rafatakitani/rege.git
cd rege
cargo install --path .
```

Either way the `rege` binary lands in `~/.cargo/bin` (make sure it is on your `PATH`).

To update later, from any directory (with no new commit on the remote it answers
straight away, compiling nothing):

```bash
rege update            # pull and reinstall the latest upstream version
rege update --branch x # or a specific branch/tag
rege update --verbose  # with cargo's raw output (quiet by default)
rege update --force    # rebuild even when already on the remote's commit
```

## Usage

```bash
rege                       # open the TUI (orchestrator with chat, agents, themes)
rege exec "fix the login bug"   # headless (like `codex exec`): orchestrates and prints
rege claude                # open claude interactive, already in Rege mode (playbook+MCP)
rege scan                  # scan the current directory and write AGENTS.md
                           # (in the TUI: /scan reads the files, /grill interviews you)
rege doctor                # health check of the CLI roster + current master
rege config                # print the effective config
rege mcp-serve --repo .    # bare MCP server (JSON-RPC over stdio) for the repo
rege render --demo         # draw one TUI frame as text (headless, no tty)
```

### TUI commands

`/help` · `/theme` (picker with live preview) · `/model <name>` · `/config` · `/resume`
(earlier sessions) · `/agents` (roster: connect/remove CLIs, saved to the config;
`/agents active` lists the running workers) · `/scan` · `/grill` · `/buddy` (animated
pet) · `/quit` (or `exit`).
Typing `/` opens an autocomplete of the commands: `↑↓` moves, `Tab` completes.

**Your own skills work here.** Anything in `~/.claude/skills`, `~/.claude/commands` or
the project's `.claude/` shows up in the same autocomplete and runs on the master —
`/explain what a worktree is` reaches `claude` verbatim. rege only forwards names it
found, so a typo is still a cheap error instead of a paid turn. (Master must be
`claude`; skills are a Claude Code feature.)
Selecting text with the mouse copies via OSC52 (works over `ssh`/`tmux` with
passthrough); turn it off with `ui.auto_copy`.

**Remote:** the TUI runs in a terminal, so from your phone or another device all you
need is `ssh` (over Tailscale, say) + `tmux attach`. No app.

## Directory context (`/scan` and `/grill`)

The first time you open rege in a directory, it asks how you want it to learn about the
place. Two routes, the same destination (`AGENTS.md`):

- **`/scan`** reads what the code already says — git, manifests, extensions, the tree —
  and writes the description in a single call. Like Claude Code's `/init`.
- **`/grill`** does the opposite: the master **interviews you**, one question at a time,
  about what you are building, what is already decided and why, and what agents must not
  touch. At the end it writes `AGENTS.md`, one `docs/adr/NNN-*.md` per decision that came
  up, and `docs/glossary.md` when the vocabulary is worth pinning down.

An untouched `rails new` is the clear case: nothing to scan, everything to ask. A repo
with years of history is the reverse. Both work in any folder: run it in `~/` and it
writes in `~/`; run it in `/economia` and it writes there.

`AGENTS.md` is deliberate: claude, codex and opencode already read that file on their
own, so the context reaches the workers without rege injecting anything into their
prompts.

How the scan works inside: rege gathers the facts itself (git, most common extensions,
manifests, build/test commands, the start of the README, the tree up to 2 levels) and
sends that summary to the master in one call. The model reads a finished digest instead
of walking the disk — cheap, and it works outside a git repository.

- Asks **once per directory**. Declined? The answer lives in
  `~/.config/rege/scanned.yml` and it won't ask there again — nothing is written into
  your project.
- An `AGENTS.md` already there? It doesn't ask, and `rege scan` refuses to overwrite
  without `--force`.
- `/scan` and `/grill` in the TUI run on demand; headless (`rege exec`, `mcp-serve`)
  never asks.
- The interview is conducted by the **master**, not by a fixed script: it is already in
  the conversation, reads the code before asking, and follows an answer where it leads.
  No worker takes part.
- In `~/` the walk is shallow and capped, and the prompt warns the model that this is a
  home directory, not a project.

## Configuration

Layers, deep-merged in this order: defaults ← `~/.config/rege/config.yml` (global) ←
`.rege.yml` (per project). Adjustable: master (`master.cli` / `master.model`), roster
(role→CLI→model), theme, `ui.auto_copy`, and more.

```yaml
# ~/.config/rege/config.yml
master:
  cli: claude
  model: sonnet   # scale up to opus on the hard steps (planner/reviewer/consult)
ui:
  theme: hacker
  auto_copy: true
```

## The two orchestration modes

- **Easy** — divide & conquer: workers take different parts → merge everything → review.
- **Hard** — redundancy & judge: several do the same thing → synthetic merge → repair
  loop (bug hunt, run the tests if any exist, 3 rounds max).

## Token thrift (`rtk`)

[`rtk`](https://github.com/rtk-ai/rtk) is a CLI proxy that filters command output before
it becomes LLM context (-60% to -90% tokens). `rege` uses it if it is on the `PATH`, with
no configuration:

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g            # Claude Code hook (workers get it too)
rtk init -g --opencode
rtk init -g --gemini --auto-patch
rtk init -g --codex
```

What `rege` routes through `rtk`:

| path | becomes | why |
|------|---------|-----|
| `diff_agent` / `review` (the worker branch diff) | `rtk git diff` | it is the biggest block entering the master's context |
| worker output (`git status`, tests, `ls`…) | the CLI's own hook | `rtk init -g` rewrites their Bash commands |

What stays **raw** on purpose: `open_pr`'s `.patch` (a condensed diff doesn't apply) and
the internal git plumbing (`rev-parse`, `worktree`, `commit`) — nobody reads that.

To compress `run_tests` too, put `rtk` in your verify command:

```yaml
verify:
  command: rtk cargo test   # instead of: cargo test
```

Precedence, most specific first: **`REGE_RTK`** > **`config.yml`** > **autodetection on
the `PATH`**.

```yaml
rtk:
  enabled: true         # absent = auto (used if the binary is on the PATH)
  hook_workers: false   # installs rtk's hook inside each worker's worktree
  clis: [claude]        # which workers get the hook
  init_args: [init, --hook-only]
```

`REGE_RTK=0 rege …` turns it off for one run; `REGE_RTK=1` forces it on even when the
binary isn't detected.

`hook_workers` is explicit opt-in and never turns on by autodetection: writing a hook
file inside somebody else's worktree is too intrusive to happen on its own. Compressing
a diff rege was going to show anyway is passive, so that one can be automatic.

## For AIs / agents

If you are an AI agent (Claude, Codex, Gemini…) operating this repo or using `rege` as a
tool, read [`AGENTS.md`](AGENTS.md) — it covers how to invoke `rege` headless, which MCP
tools exist, and the rules (workers isolated in worktrees, never merge on your own,
always open a PR).

## Development

```bash
cargo test    # 192 tests
cargo fmt && cargo clippy
```

Most of the Rust version was built by Rege itself (dogfooding): workers in separate
worktrees wrote the backend modules. The original Ruby implementation is preserved on
the `legacy-ruby` branch. Full design in
`docs/superpowers/specs/2026-07-23-rege-design.md`.

## Credits & license

- MIT — see [`LICENSE`](LICENSE).
- `/buddy` is a port of [ramarivera/claude-buddy](https://github.com/ramarivera/claude-buddy)
  (MIT, © ramarivera): species, stats and the deterministic generation algorithm.
- `/grill` borrows its shape from Matt Pocock's `grilling` and `domain-modeling` agent
  skills — one question at a time, each with a recommendation, decisions landing as ADRs
  plus a glossary. The prompt in `src/grill.rs` is written from scratch; the idea is
  theirs. (ADRs as a convention: Michael Nygard, 2011.)
