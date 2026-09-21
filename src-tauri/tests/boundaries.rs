//! Structural guards on the Host boundary.
//!
//! The rules these check are the kind that a reviewer has to notice, because
//! breaking them looks like ordinary code. They are cheap to assert and they
//! fail loudly, which is better than a convention in a document.
//!
//! Each reads the repository's own files. That is unusual for a test, but the
//! subject *is* the shape of the repository: "no SQL in the command layer" and
//! "one module owns `invoke`" are facts about files, not about values.

use std::path::{Path, PathBuf};

/// The repository root, from this crate's manifest directory.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The file with whole-line comments removed.
///
/// These guards are about what the code *does*, and documentation that states a
/// rule - "no WebSocket here, that is a later step" - must not be mistaken for
/// breaking it.
///
/// Deliberately conservative: only a line whose first non-whitespace is a
/// comment marker is dropped, so a trailing comment never takes code with it and
/// a string literal containing `//` is left intact. That matters for the SQL
/// guard, where the thing being looked for lives inside a string.
fn code_only(contents: &str) -> String {
    let mut in_block = false;
    let mut kept = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim_start();

        if in_block {
            if trimmed.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            // A single-line `/* ... */` opens and closes on the same line.
            in_block = !trimmed.contains("*/");
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        kept.push(line);
    }

    kept.join("\n")
}

/// Every file under `directory` with one of `extensions`, sorted for a stable
/// failure message.
fn sources(directory: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![directory.to_path_buf()];

    while let Some(current) = stack.pop() {
        let entries =
            std::fs::read_dir(&current).unwrap_or_else(|e| panic!("{}: {e}", current.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.contains(&e))
            {
                found.push(path);
            }
        }
    }

    found.sort();
    assert!(
        !found.is_empty(),
        "no sources found under {}",
        directory.display()
    );
    found
}

// ---------------------------------------------------------------------------
// 7. Neither the command layer nor the UI touches SQLite
// ---------------------------------------------------------------------------

#[test]
fn the_tauri_crate_cannot_execute_sql() {
    // `app-db` is a dependency because the Host owns the database *handle*, but
    // `rusqlite` is not. Without it there is no type in scope that can run a
    // statement, so "no SQL in the transport" is enforced by the dependency
    // graph rather than by review.
    let manifest = read(&repository_root().join("src-tauri/Cargo.toml"));
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a [dependencies] section")
        .split("\n[")
        .next()
        .expect("the section ends");

    for forbidden in ["rusqlite", "r2d2", "refinery", "libsqlite3"] {
        assert!(
            !dependencies.contains(forbidden),
            "src-tauri must not depend on {forbidden}: SQL belongs in app-db"
        );
    }
}

#[test]
fn no_sql_appears_in_the_tauri_crate_or_the_host_ui() {
    // A belt-and-braces check on top of the dependency graph: a raw statement
    // passed to something else, or assembled in the frontend, would still be a
    // boundary violation.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]));

    // Uppercase, with a trailing space, so ordinary prose ("select a meeting",
    // "updated_at") does not trip it.
    let statements = [
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "CREATE TABLE",
        "DROP ",
        "PRAGMA ",
    ];

    for file in files {
        let contents = code_only(&read(&file));
        for statement in statements {
            assert!(
                !contents.contains(statement),
                "{} contains `{statement}`: SQL belongs in app-db",
                file.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The command interface is centralized
// ---------------------------------------------------------------------------

#[test]
fn only_one_module_in_the_host_ui_calls_invoke() {
    // Architecture: the UI talks to one typed service layer, not to Tauri from
    // wherever it happens to be convenient. Scattering `invoke` is how an
    // untyped call, or a second copy of a command name, gets in.
    let root = repository_root();
    let gateway = root.join("apps/host-ui/src/api/hostApi.ts");
    assert!(
        gateway.is_file(),
        "the single Tauri gateway module is missing: {}",
        gateway.display()
    );

    let mut callers = Vec::new();
    for file in sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]) {
        let contents = read(&file);
        if contents.contains("@tauri-apps/api") || contents.contains("invoke(") {
            callers.push(file);
        }
    }

    assert_eq!(
        callers,
        vec![gateway],
        "only apps/host-ui/src/api/hostApi.ts may reach Tauri directly"
    );
}

#[test]
fn every_registered_command_is_reachable_from_the_host_ui_gateway() {
    // Keeps the two ends of the boundary honest: a command registered in Rust
    // but never called, or called under a name Rust does not register, is a
    // silent failure at runtime rather than at build time.
    let root = repository_root();
    let lib = read(&root.join("src-tauri/src/lib.rs"));
    let gateway = read(&root.join("apps/host-ui/src/api/hostApi.ts"));

    let handler = lib
        .split("generate_handler![")
        .nth(1)
        .expect("an invoke handler")
        .split(']')
        .next()
        .expect("the handler list ends");

    let registered: Vec<&str> = handler
        .split(',')
        .filter_map(|entry| entry.trim().strip_prefix("commands::"))
        .filter(|name| !name.is_empty())
        .collect();

    assert_eq!(
        registered.len(),
        10,
        "expected the ten step-4 commands, found {registered:?}"
    );

    for command in registered {
        assert!(
            gateway.contains(&format!("'{command}'")),
            "command `{command}` is registered in Rust but never invoked by the UI"
        );
    }
}

// ---------------------------------------------------------------------------
// Capabilities stay minimal
// ---------------------------------------------------------------------------

#[test]
fn the_window_capability_grants_nothing_beyond_the_core_defaults() {
    // PRD section 22 and the architecture rules both reduce to: do not hand the
    // frontend anything it has not been shown to need. Filesystem, shell and
    // dialog access are the usual things added "just for development".
    let capability = read(&repository_root().join("src-tauri/capabilities/default.json"));

    for forbidden in [
        "fs:", "shell:", "http:", "process:", "dialog:", "os:", "sql:", "updater:",
    ] {
        assert!(
            !capability.contains(forbidden),
            "the default capability must not grant {forbidden}"
        );
    }
    assert!(
        capability.contains("core:default"),
        "the core defaults are the whole capability set"
    );
}

#[test]
fn the_content_security_policy_is_still_restrictive() {
    // The Host UI is local and self-contained; it must not be able to load or
    // reach anything off the device (PRD section 4, section 24).
    let config = read(&repository_root().join("src-tauri/tauri.conf.json"));
    let csp = config
        .split("\"csp\"")
        .nth(1)
        .expect("a csp entry")
        .split('\n')
        .next()
        .expect("the entry ends");

    assert!(csp.contains("default-src 'self'"), "{csp}");
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert!(csp.contains("object-src 'none'"), "{csp}");
    // No remote origin may appear: no CDN, no API, no telemetry endpoint.
    assert!(!csp.contains("https://"), "{csp}");
    assert!(!csp.contains("unsafe-eval"), "{csp}");
    // `unsafe-inline` is permitted for styles only, which is what the bundler
    // emits; scripts must stay strict.
    let script_src = csp
        .split("script-src")
        .nth(1)
        .expect("script-src")
        .split(';')
        .next()
        .expect("the directive ends");
    assert!(!script_src.contains("unsafe-inline"), "{script_src}");
}

// ---------------------------------------------------------------------------
// The audit log is read-only from this boundary
// ---------------------------------------------------------------------------

#[test]
fn no_command_offers_to_change_an_audit_entry() {
    // The database refuses UPDATE and DELETE on `audit_logs` (ADR-0011). This
    // asserts the layer above does not even offer the shape of such an action,
    // so the refusal is never something a user gets to see.
    let commands = read(&repository_root().join("src-tauri/src/commands.rs"));

    for forbidden in [
        "fn update_audit",
        "fn delete_audit",
        "fn remove_audit",
        "fn write_audit",
        "fn create_audit",
    ] {
        assert!(
            !commands.contains(forbidden),
            "the audit log is append-only: `{forbidden}` must not exist"
        );
    }
    assert!(
        commands.contains("fn list_audit_entries"),
        "reading the audit log is the only audit command"
    );
}

// ---------------------------------------------------------------------------
// Scope: the LAN transport is not in this step
// ---------------------------------------------------------------------------

#[test]
fn no_lan_transport_has_crept_into_the_host_shell() {
    // Step 4 is the Host surface only. These names are the ones that would
    // appear first if the LAN server, its join token or its sockets had started
    // to leak into the shell ahead of their own step.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]));

    for file in files {
        let contents = code_only(&read(&file));
        for forbidden in [
            "axum",
            "WebSocket",
            "websocket",
            "join_token",
            "joinToken",
            "qrcode",
            "QrCode",
            "listen(",
            "TcpListener",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: the LAN transport is a later step",
                file.display()
            );
        }
    }
}
