//! Makes the embedded-asset directory exist before `rust-embed` looks for it.
//!
//! `apps/lan-ui/dist` is a build output and is not committed, so on a fresh
//! clone it does not exist - and `RustEmbed` fails to *compile* against a
//! missing folder. Without this, `cargo test --workspace` would fail on a clean
//! checkout for a reason that has nothing to do with the code being tested.
//!
//! Creating the directory rather than requiring the bundle keeps the two build
//! systems independent: Rust always compiles, and whether the real bundle is
//! present is a separate, checkable fact (`assets::is_bundled`). The validation
//! sequence builds `lan-ui` first, so the committed artefact is always the real
//! one.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dist = manifest
        .join("..")
        .join("..")
        .join("apps")
        .join("lan-ui")
        .join("dist");

    if !dist.is_dir() {
        std::fs::create_dir_all(&dist).unwrap_or_else(|e| {
            panic!(
                "could not create the LAN UI asset directory at {}: {e}. \
                 Run `npm run build:lan` to produce the real bundle.",
                dist.display()
            )
        });
    }

    // Re-embed when the bundle changes, so a rebuilt UI reaches the binary
    // without a `cargo clean`.
    println!("cargo:rerun-if-changed={}", dist.display());
}
