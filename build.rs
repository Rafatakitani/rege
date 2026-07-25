//! Stamps the build's git commit into the binary, so `rege --version` answers
//! "did my update actually land?" — a bare semver can't, since it only changes
//! when someone remembers to bump it.

use std::process::Command;

fn main() {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty());

    // `cargo install --git` builds from a clone, so this normally resolves; a
    // tarball or vendored build has no git and gets a clear placeholder.
    println!("cargo:rustc-env=REGE_GIT_HASH={}", hash.unwrap_or_else(|| "sem-git".to_string()));

    // Rebuild when HEAD moves, otherwise the stamp goes stale on the next
    // commit — the persistent target dir makes that much more likely.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
