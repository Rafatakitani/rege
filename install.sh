#!/bin/sh
# rege installer — one command, working binary, even on a machine without Rust.
#
#   curl -fsSL https://raw.githubusercontent.com/Rafatakitani/rege/main/install.sh | sh
#
# What it does, in order: checks git, installs Rust via rustup when cargo is
# missing (user-space, no sudo), compiles rege from the upstream repo the same
# way `rege update` does (same profile, same build cache — so the first
# `rege update` later starts warm), and checks for tmux at the end.
# REGE_BRANCH=x pins a branch/tag; everything else has no knobs.
set -eu

REGE_GIT_URL="https://github.com/Rafatakitani/rege.git"

say() { printf '%s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# The one package manager this machine answers to, or nothing.
pkg_hint() {
    if have pacman; then say "  sudo pacman -S $1"
    elif have apt-get; then say "  sudo apt-get install $1"
    elif have dnf; then say "  sudo dnf install $1"
    elif have zypper; then say "  sudo zypper install $1"
    elif have apk; then say "  sudo apk add $1"
    elif have brew; then say "  brew install $1"
    else say "  (install $1 with your system's package manager)"
    fi
}

# git first: cargo's fetch, the worktrees and rege itself all need it, and
# rustup can't install it for us.
if ! have git; then
    say "rege needs git and it is not installed. Install it and rerun:"
    pkg_hint git
    exit 1
fi

# Rust. rustup already on disk but not on this shell's PATH counts as present.
PATH="$HOME/.cargo/bin:$PATH"
if ! have cargo; then
    say "cargo not found — installing Rust via rustup (lives in ~/.cargo, no sudo)…"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    # rustup wired up bash/zsh/fish for future shells; this one still needs it.
    . "$HOME/.cargo/env"
fi

# Same knobs as `rege update`: the fastinstall profile optimizes build time
# over binary speed, --locked installs the versions that were actually tested,
# the shared target dir means update reuses today's artifacts, and delegating
# the fetch to the git CLI dodges libgit2's DNS quirks.
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_TARGET_DIR="$HOME/.cache/rege/build"
set -- --profile fastinstall --locked
[ -n "${REGE_BRANCH:-}" ] && set -- "$@" --branch "$REGE_BRANCH"
say "compiling rege… (~1 min on a warm machine, longer on the first Rust build)"
if ! cargo install --git "$REGE_GIT_URL" --force "$@"; then
    say "the build failed — the error is right above this line."
    exit 1
fi

# tmux is a runtime dependency (each worker lives in a tmux session), not a
# build one, so its absence warns instead of aborting.
if ! have tmux; then
    say ""
    say "one thing left: rege runs its workers inside tmux, and tmux is not installed:"
    pkg_hint tmux
fi

say ""
"$HOME/.cargo/bin/rege" --version
say "installed to ~/.cargo/bin/rege — open a new terminal if the command is not found."
say "next: run \`rege doctor\` to check which AI CLIs it can see."
