//! Makes the embedded-template directory exist before `rust-embed` looks for it.
//!
//! `apps/remote-form/dist` is a build output and is not committed, so on a
//! fresh clone it does not exist - and `RustEmbed` fails to *compile* against a
//! missing folder. Without this, `cargo test --workspace` would fail on a clean
//! checkout for a reason that has nothing to do with the code being tested.
//!
//! Creating the directory rather than requiring the template keeps the two
//! build systems independent: Rust always compiles, and whether the real
//! template is present is a separate, checkable fact
//! ([`app_remote::generate::is_bundled`]). The validation sequence builds
//! `remote-form` first, so the artefact a Host ships is always the real one.
//!
//! This mirrors `crates/app-server/build.rs` deliberately. Two embedded bundles,
//! one pattern.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dist = manifest
        .join("..")
        .join("..")
        .join("apps")
        .join("remote-form")
        .join("dist");

    if !dist.is_dir() {
        std::fs::create_dir_all(&dist).unwrap_or_else(|e| {
            panic!(
                "could not create the remote form template directory at {}: {e}. \
                 Run `npm run build:remote` to produce the real template.",
                dist.display()
            )
        });
    }

    // Re-embed when the template changes, so a rebuilt form reaches the binary
    // without a `cargo clean`.
    println!("cargo:rerun-if-changed={}", dist.display());
}
