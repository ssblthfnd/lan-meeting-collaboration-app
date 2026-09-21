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

/// The code that ships, with comments and the trailing test module removed.
///
/// The boundary guards are about what the shipped transport *does*. A test
/// fixture is neither: `app-server` deliberately hashes `"'; DROP TABLE
/// meetings; --"` to prove that a hostile token is a lookup that matches
/// nothing, and that string is evidence the boundary holds rather than evidence
/// it was crossed.
///
/// Every file in this repository places its `#[cfg(test)]` module last and has
/// exactly one, which this asserts rather than assumes - if that ever stops
/// being true, this returns a wrong answer silently, so it should fail loudly
/// instead.
fn shipped_code(path: &Path, contents: &str) -> String {
    let markers = contents
        .lines()
        .filter(|line| line.trim_start().starts_with("#[cfg(test)]"))
        .count();
    assert!(
        markers <= 1,
        "{}: expected at most one `#[cfg(test)]` module, found {markers}. \
         The boundary scan truncates at the first one and would miss code after it.",
        path.display()
    );

    let shipped: String = contents
        .lines()
        .take_while(|line| !line.trim_start().starts_with("#[cfg(test)]"))
        .collect::<Vec<_>>()
        .join("\n");

    code_only(&shipped)
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
fn no_sql_appears_in_either_transport_or_the_host_ui() {
    // A belt-and-braces check on top of the dependency graph: a raw statement
    // passed to something else, or assembled in a frontend, would still be a
    // boundary violation.
    //
    // Both transports are scanned. `app-server` is the one that faces untrusted
    // input, so leaving it out would have left the gap in the crate where it
    // matters most.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("crates/app-server/src"), &["rs"]));
    files.extend(sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]));
    files.extend(sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]));

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
        let contents = shipped_code(&file, &read(&file));
        for statement in statements {
            assert!(
                !contents.contains(statement),
                "{} contains `{statement}`: SQL belongs in app-db",
                file.display()
            );
        }
    }
}

#[test]
fn no_transport_reaches_for_a_database_primitive() {
    // The gap the SQL scan alone leaves open.
    //
    // `Db::write`, `Db::write_with` and `Db::with_writer` are public, and both
    // transports hold a `Db`. The closure they pass receives a transaction
    // whose methods can be called *without naming its type*, so the absence of
    // a `rusqlite` dependency does not by itself stop a transport from writing:
    //
    //     db.write(|tx| { tx.execute("...", [])?; Ok(()) })
    //
    // That would bypass `Domain` entirely - no authorization proof, no lock
    // check, no audit record - and every other test in this suite would still
    // pass. So the primitives are named here and forbidden outright.
    //
    // Reads are no exception. A transport reads through the typed query types
    // (`HostQueries`, `ParticipantQueries`), which is what keeps the two
    // audiences separated (ADR-0014); reaching past them to a raw connection
    // would defeat that just as surely.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("crates/app-server/src"), &["rs"]));

    let primitives = [
        ".write(",
        ".write_with(",
        ".with_writer(",
        ".read(",
        "Transaction",
        "Connection",
        "execute(",
        "query_row(",
        "query_map(",
        ".prepare(",
    ];

    for file in files {
        let contents = shipped_code(&file, &read(&file));
        for primitive in primitives {
            assert!(
                !contents.contains(primitive),
                "{} uses `{primitive}`: a transport must reach the database only \
                 through app-core::Domain or a typed query, never a raw primitive",
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
        17,
        "every registered command must be reachable, found {registered:?}"
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
// Scope: realtime and remote participation are later steps
// ---------------------------------------------------------------------------

#[test]
fn no_realtime_or_relay_code_has_crept_in() {
    // The LAN transport arrived in step 6; WebSocket, presence and remote
    // participation did not. These are the names that would appear first if a
    // later step had started to leak backwards, and the "no cloud, no relay"
    // promise had started to erode.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("crates/app-server/src"), &["rs"]));
    files.extend(sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]));
    files.extend(sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]));

    for file in files {
        let contents = code_only(&read(&file));
        for forbidden in [
            "WebSocket",
            "websocket",
            "tokio_tungstenite",
            "EventSource",
            "setInterval",
            // No cloud, relay, tunnel or public hosting. Ever (PRD section 4).
            // These are outbound-client and port-opening names: the absence of
            // a remote *origin* is enforced separately, by the content security
            // policy the server sends and the tests over it.
            "reqwest",
            "ureq",
            "hyper::Client",
            "ngrok",
            "upnp",
            "UPnP",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: out of scope for this step",
                file.display()
            );
        }
    }
}

#[test]
fn the_lan_transport_cannot_execute_sql_either() {
    // The same guarantee `src-tauri` has, for the crate that faces untrusted
    // input: `app-server` holds `app-db` for its query types, and no SQLite
    // driver, so there is no type in scope that can run a statement.
    let manifest = read(&repository_root().join("crates/app-server/Cargo.toml"));
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a [dependencies] section")
        .split(
            "
[",
        )
        .next()
        .expect("the section ends");

    for forbidden in ["rusqlite", "r2d2", "refinery", "libsqlite3"] {
        assert!(
            !dependencies.contains(forbidden),
            "app-server must not depend on {forbidden}: SQL belongs in app-db"
        );
    }
}

#[test]
fn the_lan_server_embeds_only_the_participant_bundle() {
    // `apps/host-ui` is the Host's own window and talks to Tauri commands a
    // browser has no business reaching. It must never be served over the
    // network (CLAUDE.md, the three UI bundles).
    // Comments stripped: this file's documentation explains *why* host-ui is
    // never embedded, and saying so must not read as doing so.
    let assets = code_only(&read(
        &repository_root().join("crates/app-server/src/assets.rs"),
    ));

    let folders: Vec<&str> = assets
        .lines()
        .filter(|line| line.trim_start().starts_with("#[folder"))
        .collect();

    assert_eq!(folders.len(), 1, "exactly one embedded folder: {folders:?}");
    assert!(
        folders[0].contains("apps/lan-ui/dist"),
        "the embedded bundle must be the participant one: {}",
        folders[0]
    );
    assert!(!assets.contains("host-ui"), "host-ui must not be embedded");
}

#[test]
fn the_participant_ui_never_reaches_for_tauri() {
    // The LAN bundle runs in someone else's browser. It has no Tauri API, and
    // asking for one would mean a capability had been confused for a transport.
    let root = repository_root();
    let gateway = root.join("apps/lan-ui/src/api/lanApi.ts");
    assert!(
        gateway.is_file(),
        "the single LAN gateway module is missing"
    );

    let mut callers = Vec::new();
    for file in sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]) {
        let contents = read(&file);
        assert!(
            !contents.contains("@tauri-apps"),
            "{} imports the Tauri API",
            file.display()
        );
        if contents.contains("fetch(") {
            callers.push(file);
        }
    }

    assert_eq!(
        callers,
        vec![gateway],
        "only apps/lan-ui/src/api/lanApi.ts may call fetch"
    );
}

#[test]
fn a_session_token_is_never_written_to_storage_outside_one_module() {
    // The participant's only credential (ADR-0002 rule 6). One module reads and
    // writes it, so "where does the token live" has one answer.
    let root = repository_root();
    let keeper = root.join("apps/lan-ui/src/api/session.ts");

    let mut touching = Vec::new();
    for file in sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]) {
        let contents = code_only(&read(&file));
        if contents.contains("sessionStorage")
            || contents.contains("localStorage")
            || contents.contains("document.cookie")
        {
            touching.push(file);
        }
    }

    assert_eq!(
        touching,
        vec![keeper],
        "only apps/lan-ui/src/api/session.ts may touch browser storage"
    );
}
