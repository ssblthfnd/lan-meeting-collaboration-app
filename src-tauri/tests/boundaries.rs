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
        30,
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
// Note content is rendered, never injected
// ---------------------------------------------------------------------------

#[test]
fn no_bundle_assigns_html_directly() {
    // Participant note content is hostile input even in the Host's own window
    // (PRD 13.3). The renderer in `packages/editor` builds DOM nodes and sets
    // text through `textContent`, so there is no sink to inject into - and
    // this is what stops one being introduced beside it.
    let root = repository_root();
    let mut files = sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]);
    files.extend(sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]));
    files.extend(sources(&root.join("apps/remote-form/src"), &["ts"]));
    files.extend(sources(&root.join("packages/editor/src"), &["ts"]));

    for file in files {
        let contents = code_only(&read(&file));
        for forbidden in [
            "innerHTML",
            "outerHTML",
            "dangerouslySetInnerHTML",
            "insertAdjacentHTML",
            "document.write",
            "createContextualFragment",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} uses `{forbidden}`: note content is rendered as DOM nodes, never as markup",
                file.display()
            );
        }
    }
}

#[test]
fn only_the_shared_editor_renders_markdown() {
    // ADR-0007 puts the editor and renderer in one package so the three
    // bundles cannot drift into three dialects of the same format. A second
    // renderer in a component would be that drift starting, and it would be
    // the copy without the safety argument behind it.
    let root = repository_root();
    let mut files = sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]);
    files.extend(sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]));

    for file in files {
        let contents = code_only(&read(&file));
        // Matched as an import rather than as a word: a component may well
        // say "marked" in a sentence, and prose is not a dependency.
        for forbidden in ["marked", "markdown-it", "remark", "showdown", "micromark"] {
            for form in [
                format!("from '{forbidden}'"),
                format!("require('{forbidden}')"),
            ] {
                assert!(
                    !contents.contains(&form),
                    "{} imports `{forbidden}`: Markdown is rendered by packages/editor",
                    file.display()
                );
            }
        }
        // A component may *call* the shared renderer; it may not reimplement
        // one. Parsing Markdown structure is what an ad-hoc renderer looks
        // like before it grows.
        for forbidden in ["parseMarkdown(", "renderBlocks("] {
            assert!(
                !contents.contains(forbidden),
                "{} calls `{forbidden}`: a bundle renders through renderMarkdown, \
                 and does not walk the document model itself",
                file.display()
            );
        }
    }
}

#[test]
fn the_shared_editor_stays_bundleable_into_the_offline_form() {
    // `packages/editor` is bundled into `apps/remote-form`, whose build guard
    // reads the built bytes and refuses any absolute URL and any network call.
    // Catching it here means the breakage surfaces in the step that wrote the
    // code rather than in the step that bundles it - and the offline guard is
    // never relaxed to accommodate this package (ADR-0009).
    let root = repository_root();

    for file in sources(&root.join("packages/editor/src"), &["ts"]) {
        let contents = code_only(&read(&file));

        assert!(
            !contents.contains("http://") && !contents.contains("https://"),
            "{} contains an absolute URL literal: the offline form's guard \
             refuses one in the built bytes",
            file.display()
        );

        for forbidden in [
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "EventSource",
            "sendBeacon",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: the editor makes no network call",
                file.display()
            );
        }

        for framework in [
            "from 'react'",
            "from 'react-dom'",
            "from 'vue'",
            "from 'svelte'",
        ] {
            assert!(
                !contents.contains(framework),
                "{} imports a framework: the editor must work without one (ADR-0009)",
                file.display()
            );
        }
    }

    // And it declares no runtime dependency, so nothing can arrive through the
    // back door of a transitive install.
    let manifest = read(&root.join("packages/editor/package.json"));
    assert!(
        !manifest.contains("\"dependencies\""),
        "packages/editor must declare no runtime dependency"
    );
}

// ---------------------------------------------------------------------------
// Scope: history is view-only, and participant note editing is a later step
// ---------------------------------------------------------------------------

#[test]
fn nothing_restores_a_note_version() {
    // Step 8 decision D1: version history is view-only. `note_versions` is
    // append-only and the database refuses `UPDATE` on it, but the thing to
    // prevent is not an edit of history - it is a command that writes a
    // historical body back as the current note, which is the whole of restore
    // arriving without the decision being made.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("crates/app-core/src"), &["rs"]));
    files.extend(sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]));

    for file in files {
        let contents = code_only(&read(&file));
        for forbidden in [
            "restore_note",
            "restoreNote",
            "RestoreNoteVersion",
            "NoteRestored",
            "note.restored",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: note restore is deferred (ADR-0019)",
                file.display()
            );
        }
    }
}

#[test]
fn the_participant_note_route_addresses_nobody() {
    // Step 8B's whole security argument in one assertion. `/api/note` takes no
    // path segment, so there is no identifier to substitute; the meeting and
    // the participant come from the resolved session. A check can be forgotten
    // by a future handler, and a parameter that does not exist cannot be
    // (ADR-0020).
    let root = repository_root();
    let router = code_only(&read(&root.join("crates/app-server/src/router.rs")));

    assert!(
        router.contains("\"/api/note\""),
        "the participant note route is missing"
    );
    for forbidden in [
        "/api/note/{",
        "/api/meetings/{",
        "/api/participants/{",
        "/api/note/{participant_id}",
    ] {
        assert!(
            !router.contains(forbidden),
            "the note route must carry no identifier, found `{forbidden}`"
        );
    }

    // And the request body has one field. A `participant_id` here would be a
    // value the server could read by mistake; there is none to read.
    let dto = shipped_code(
        &root.join("crates/app-server/src/dto.rs"),
        &read(&root.join("crates/app-server/src/dto.rs")),
    );
    let request = dto
        .split("pub struct WriteNoteRequest")
        .nth(1)
        .expect("a WriteNoteRequest")
        .split('}')
        .next()
        .expect("the struct ends");
    for forbidden in [
        "participant_id",
        "meeting_id",
        "note_id",
        "expected_version",
    ] {
        assert!(
            !request.contains(forbidden),
            "WriteNoteRequest must not carry `{forbidden}`"
        );
    }
}

#[test]
fn the_participant_note_response_carries_no_identifiers() {
    // The participant shape is four fields, and the omissions are the design:
    // no note id, no author id, no meeting or participant id (ADR-0020).
    let root = repository_root();
    let dto = read(&root.join("crates/app-server/src/dto.rs"));
    let view = dto
        .split("pub struct NoteView")
        .nth(1)
        .expect("a NoteView")
        .split('}')
        .next()
        .expect("the struct ends");

    for forbidden in [
        "note_id",
        "last_author_id",
        "meeting_id",
        "participant_id",
        "session_id",
    ] {
        assert!(
            !view.contains(forbidden),
            "NoteView must not expose `{forbidden}`"
        );
    }
}

#[test]
fn the_participant_read_path_stays_its_own() {
    // ADR-0014: read models are named for their audience. The participant note
    // read must not be the Host's query wearing a different name, because the
    // Host's shape carries fields a participant may not need to see.
    let root = repository_root();
    let server = sources(&root.join("crates/app-server/src"), &["rs"]);

    for file in server {
        let contents = shipped_code(&file, &read(&file));
        assert!(
            !contents.contains("HostQueries"),
            "{} reaches for the Host's read model",
            file.display()
        );
    }
}

#[test]
fn the_note_body_limit_is_scoped_to_the_write_method() {
    // The kilobyte is what stops an untrusted client making the Host allocate
    // for an identifier (architecture rules section 10). A note needs more, and
    // it gets it on one *method* of one route - not globally, and not on the
    // read, which buffers no body at all (ADR-0020).
    let path = repository_root().join("crates/app-server/src/router.rs");
    let router = shipped_code(&path, &read(&path));

    assert!(
        router.contains("const MAX_BODY_BYTES: usize = 1024;"),
        "the global body limit must stay at a kilobyte"
    );

    // Each `.route(` registration as written, cut at the next one. The
    // override belongs to a registration, not to the router: a
    // `DefaultBodyLimit` on the outer builder would raise it for everything,
    // including the asset fallback.
    let registrations: Vec<&str> = router.split(".route(").skip(1).collect();
    let note: Vec<&&str> = registrations
        .iter()
        .filter(|registration| registration.contains("\"/api/note\""))
        .collect();

    assert_eq!(
        note.len(),
        2,
        "GET and PUT must be registered separately, or the limit cannot differ          between them: {note:#?}"
    );

    let write = note
        .iter()
        .find(|registration| registration.contains("put(routes::write_note)"))
        .expect("a registration for the note write");
    let read_note = note
        .iter()
        .find(|registration| registration.contains("get(routes::read_note)"))
        .expect("a registration for the note read");

    assert!(
        write.contains("DefaultBodyLimit::max(MAX_NOTE_BODY_BYTES)"),
        "the larger limit must be attached to the PUT itself: {write}"
    );
    assert!(
        !write.contains("get(routes::read_note)"),
        "the PUT registration must not also carry the read, which would put          both methods behind one limit again: {write}"
    );
    assert!(
        !read_note.contains("DefaultBodyLimit"),
        "GET /api/note reads no body and must keep the global kilobyte:          {read_note}"
    );
}

#[test]
fn the_participant_bundle_has_no_note_history_and_no_identifiers_in_its_calls() {
    // Version history is the Host's (PRD section 17), and the participant
    // bundle sends no identity: the session is the authority.
    let root = repository_root();

    for file in sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]) {
        let contents = code_only(&read(&file));
        for forbidden in [
            "note_versions",
            "listNoteVersions",
            "getNoteVersion",
            "noteHistory",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: note history is the Host's (PRD section 17)",
                file.display()
            );
        }
    }

    // The note calls specifically send no identity. `claimIdentity` is the
    // deliberate exception and is not covered here: on the claim route a
    // `participant_id` is a *target* chosen from a list, and the join token is
    // the authority (ADR-0002, ADR-0016). On the note route there is no such
    // thing to name.
    let api = code_only(&read(&root.join("apps/lan-ui/src/api/lanApi.ts")));
    let note_calls = api
        .split("export function fetchOwnNote")
        .nth(1)
        .expect("the note calls");
    for forbidden in [
        "participant_id",
        "meeting_id",
        "note_id",
        "expected_version",
    ] {
        assert!(
            !note_calls.contains(forbidden),
            "the participant note calls must send no identity, found `{forbidden}`"
        );
    }
}

#[test]
fn note_links_are_still_only_a_schema() {
    // Step 8 decision D2. The table, its scheme CHECK and its five-link
    // triggers exist from step 1; what is deferred is any code that writes
    // them, because `note_versions` versions content only and what a version
    // means for links is an unanswered question.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("crates/app-core/src"), &["rs"]));

    for file in files {
        let contents = code_only(&read(&file));
        // `NoteLinkId` is deliberately not in this list: the identifier type
        // has existed since step 1 alongside the table. What is deferred is
        // code that reads or writes the rows.
        for forbidden in [
            "note_links",
            "WriteNoteLinks",
            "insert_note_link",
            "NoteLinkRow",
            "NoteLinkDto",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} touches note links: structured link editing is deferred (ADR-0019)",
                file.display()
            );
        }
    }
}

#[test]
fn no_export_renderer_has_arrived_early() {
    // Export is step 12, and the Rust Markdown renderer lands with it. Step 8
    // added a *validator*, which is a scanner rather than a parser, and the
    // distinction is worth keeping visible.
    let root = repository_root();
    let manifest = read(&root.join("crates/app-core/Cargo.toml"));

    for forbidden in ["pulldown-cmark", "comrak", "markdown", "printpdf", "genpdf"] {
        assert!(
            !manifest.contains(forbidden),
            "app-core must not depend on {forbidden}: the Rust renderer is step 12"
        );
    }
}

// ---------------------------------------------------------------------------
// Scope: step 9 generates a remote form, and does not import one
// ---------------------------------------------------------------------------

#[test]
fn the_remote_crate_holds_no_database_and_no_sql() {
    // `app-remote` reads untrusted files. It must not also be able to write to
    // the database: the import transaction is `app-core`'s, reached from
    // `src-tauri`, and a crate that parses a submission is not the place for a
    // connection (architecture rules sections 10 and 11).
    let root = repository_root();

    let manifest = read(&root.join("crates/app-remote/Cargo.toml"));
    for forbidden in ["app-db", "rusqlite", "r2d2", "refinery"] {
        assert!(
            !manifest.contains(forbidden),
            "app-remote must not depend on {forbidden}: it has no database"
        );
    }

    for file in sources(&root.join("crates/app-remote/src"), &["rs"]) {
        let contents = shipped_code(&file, &read(&file));
        for forbidden in [
            "SELECT ",
            "INSERT ",
            "UPDATE ",
            "DELETE ",
            "BEGIN ",
            "COMMIT",
            "Connection",
            "DomainTx",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: app-remote executes no SQL",
                file.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Scope: remote submission import is the Host's, and nobody else's
// ---------------------------------------------------------------------------

#[test]
fn only_the_tauri_host_layer_constructs_the_remote_import_actor() {
    // ADR-0022 decision 3. The crate that reads an untrusted file is not the
    // crate that produces authority, and neither is the LAN transport. The
    // actor is built in one function, from rows the database returned.
    let root = repository_root();

    for file in sources(&root.join("crates/app-remote/src"), &["rs"]) {
        let contents = shipped_code(&file, &read(&file));
        assert!(
            !contents.contains("Actor::"),
            "{} constructs an actor: authority is established in src-tauri, \
             not in the crate that parses untrusted input",
            file.display()
        );
    }

    for file in sources(&root.join("crates/app-server/src"), &["rs"]) {
        let contents = shipped_code(&file, &read(&file));
        assert!(
            !contents.contains("Actor::RemoteImport"),
            "{} constructs Actor::RemoteImport: import is the Host's, and a \
             participant must not be able to reach it",
            file.display()
        );
    }

    // Exactly one construction site in the shell, and it is the Host adapter.
    let mut sites = Vec::new();
    for file in sources(&root.join("src-tauri/src"), &["rs"]) {
        if shipped_code(&file, &read(&file)).contains("Actor::RemoteImport {") {
            sites.push(file.display().to_string());
        }
    }
    assert_eq!(
        sites.len(),
        1,
        "Actor::RemoteImport must be constructed in exactly one place, found {sites:?}"
    );
    assert!(
        sites[0]
            .replace('\\', "/")
            .ends_with("src-tauri/src/host.rs"),
        "the actor must be built in the Host adapter, found {sites:?}"
    );
}

#[test]
fn no_lan_route_can_reach_the_import() {
    // Import is the Host's. A participant holding a session token must not be
    // able to write somebody's note by presenting a file (ADR-0022).
    let root = repository_root();
    let router = code_only(&read(&root.join("crates/app-server/src/router.rs")));

    for forbidden in ["submission", "import"] {
        assert!(
            !router.contains(forbidden),
            "the LAN router mentions `{forbidden}`: there is no import route"
        );
    }

    for file in sources(&root.join("crates/app-server/src"), &["rs"]) {
        let contents = shipped_code(&file, &read(&file));
        for forbidden in [
            "remote_submissions",
            "SubmissionV1",
            "import_remote_submission",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} reaches for the import pipeline: it is the Host's",
                file.display()
            );
        }
    }
}

#[test]
fn an_import_is_one_transaction_and_reuses_the_note_write() {
    // ADR-0022 decision 5, the invariant the whole step turns on. Calling the
    // public `write_note` and then writing the ledger would be two
    // transactions, and could leave an imported note with no ledger row.
    let root = repository_root();
    let service = read(&root.join("crates/app-core/src/service.rs"));

    let import = service
        .split("pub fn import_remote_submission")
        .nth(1)
        .expect("the import operation")
        .split(
            "
/// The gate every note write passes",
        )
        .next()
        .expect("the operation ends");

    assert!(
        !import.contains("self.write_note("),
        "import must not call write_note as a separate transaction"
    );
    assert_eq!(
        import.matches("self.db.transaction(").count(),
        1,
        "an import opens exactly one transaction"
    );
    for required in [
        "authorize_note_write(",
        "find_remote_submission(",
        "find_foreign_remote_submission(",
        "apply_note_write(",
        "insert_remote_submission(",
        "insert_audit(",
    ] {
        assert!(
            import.contains(required),
            "the import transaction must call `{required}`"
        );
    }

    // The event follows the commit, never precedes it.
    let publish = import.find("self.publish(").expect("an event is published");
    let committed = import
        .find("committed(outcome")
        .expect("the transaction commits");
    assert!(
        committed < publish,
        "note.changed must be published only after the commit"
    );

    // Both callers share the mutation, so lock enforcement and version
    // derivation exist in one implementation.
    let write_note = service
        .split("pub fn write_note(")
        .nth(1)
        .expect("write_note")
        .split("pub fn import_remote_submission")
        .next()
        .expect("it ends");
    for required in ["authorize_note_write(", "apply_note_write("] {
        assert!(
            write_note.contains(required),
            "write_note must use the shared `{required}`"
        );
    }

    let gate = service
        .split("fn authorize_note_write(")
        .nth(1)
        .expect("the gate")
        .split("fn apply_note_write(")
        .next()
        .expect("it ends");
    for required in [
        "find_meeting(",
        "authorize(actor",
        "ensure_mutable()",
        "validate_note_content(",
        "find_participant(",
    ] {
        assert!(gate.contains(required), "the gate must still `{required}`");
    }

    let apply = service
        .split("fn apply_note_write(")
        .nth(1)
        .expect("the write");
    for required in [
        "latest_note_version(",
        "insert_note_version(",
        "insert_note(",
    ] {
        assert!(
            apply.contains(required),
            "the shared write must still `{required}`"
        );
    }
    assert!(
        !apply.contains("insert_audit("),
        "audit belongs to each caller, so an import records the action it is"
    );
}

#[test]
fn the_import_gate_decides_before_artefact_identity() {
    // The ordering a Step 10 review found reversed, pinned so it cannot regress
    // silently. A locked meeting must be refused *as locked* even when the file
    // is also a duplicate, and an actor with no standing must be refused *as
    // unauthorized* even when the artefact is one this application has seen.
    //
    // Behavioural proof lives in `crates/app-db/tests/remote_import.rs`; this is
    // the structural half, because an ordering is easy to undo by moving three
    // lines and every one of those tests would still fail only by answering
    // differently.
    let root = repository_root();
    let service = read(&root.join("crates/app-core/src/service.rs"));

    let import = service
        .split("pub fn import_remote_submission")
        .nth(1)
        .expect("the import operation")
        .split(
            "
/// The gate every note write passes",
        )
        .next()
        .expect("the operation ends");

    let gate = import
        .find("authorize_note_write(")
        .expect("the gate runs inside the transaction");

    for identity in ["find_remote_submission(", "find_foreign_remote_submission("] {
        let at = import
            .find(identity)
            .unwrap_or_else(|| panic!("`{identity}` must be called"));
        assert!(
            gate < at,
            "`{identity}` runs before the gate: the lock, the actor's authority              and the participant's membership must all decide first"
        );
    }

    // And the write comes after both, so a refusal never depends on rollback.
    let apply = import
        .find("apply_note_write(")
        .expect("the note is written");
    assert!(
        import
            .find("find_foreign_remote_submission(")
            .expect("identity")
            < apply,
        "artefact identity must be settled before anything is written"
    );
}

#[test]
fn a_refused_submission_is_never_recorded() {
    // ADR-0022 decision 7: a refusal writes nothing. The two rejection words in
    // the schema's CHECK stay unused, and nothing in shipped code reaches for
    // them.
    let root = repository_root();
    let mut files = sources(&root.join("crates/app-core/src"), &["rs"]);
    files.extend(sources(&root.join("crates/app-db/src"), &["rs"]));
    files.extend(sources(&root.join("src-tauri/src"), &["rs"]));

    for file in files {
        let contents = shipped_code(&file, &read(&file));
        for forbidden in ["REJECTED_DUPLICATE", "REJECTED_INVALID"] {
            assert!(
                !contents.contains(forbidden),
                "{} writes a rejected submission: a refusal records nothing",
                file.display()
            );
        }
    }

    // The schema still permits them, so a later design can revisit the ledger
    // deliberately rather than needing a migration to start.
    let schema = read(&root.join("crates/app-db/migrations/V1__initial_schema.sql"));
    assert!(
        schema.contains("REJECTED_DUPLICATE"),
        "the CHECK is unchanged"
    );
}

#[test]
fn the_window_never_names_a_file_or_an_artefact() {
    // ADR-0022 decisions 4 and 17. A renderer that cannot name a path cannot
    // read an arbitrary file; one that cannot hand back an artefact cannot
    // confirm one the Host never saw.
    let root = repository_root();
    let gateway = read(&root.join("apps/host-ui/src/api/hostApi.ts"));

    for command in ["preview_remote_submission", "confirm_remote_submission"] {
        let call = gateway
            .split(&format!("'{command}'"))
            .nth(1)
            .expect("the call")
            .split("}")
            .next()
            .expect("the argument object ends");

        for forbidden in ["path", "directory", "raw", "json", "artifact", "text"] {
            assert!(
                !call.contains(forbidden),
                "the window passed `{forbidden}` to {command}: the backend holds the artefact"
            );
        }
    }

    // The Rust side agrees: neither command takes bytes or a path.
    let commands = read(&root.join("src-tauri/src/commands.rs"));
    for command in [
        "pub fn preview_remote_submission",
        "pub fn confirm_remote_submission",
    ] {
        let signature = commands
            .split(command)
            .nth(1)
            .expect("the command")
            .split(')')
            .next()
            .expect("the signature ends");
        assert!(
            !signature.contains("path") && !signature.contains("raw"),
            "{command} must take no artefact and no path: {signature}"
        );
    }
}

#[test]
fn the_drag_drop_handler_holds_no_logic() {
    // `run()` cannot be tested, so nothing decidable may live in it
    // (ADR-0022 decision 17).
    let root = repository_root();
    let lib = read(&root.join("src-tauri/src/lib.rs"));

    let handler = lib
        .split(".on_window_event(")
        .nth(1)
        .expect("the drag-drop hook")
        .split(".invoke_handler(")
        .next()
        .expect("the closure ends");

    assert!(
        handler.contains("remote_import::announce_drop("),
        "the handler must delegate to a testable function"
    );
    for forbidden in ["std::fs", "read_to_string", "SubmissionV1", "Actor::"] {
        assert!(
            !handler.contains(forbidden),
            "the drag-drop handler mentions `{forbidden}`: it must only delegate"
        );
    }
}

#[test]
fn the_import_reads_no_unbounded_file() {
    // ADR-0022 decision 16. Checked before the read and again on the result, so
    // a file that grows mid-operation cannot get past the bound.
    let root = repository_root();
    let module = read(&root.join("src-tauri/src/remote_import.rs"));

    let bounded = module
        .split("pub fn read_bounded(")
        .nth(1)
        .expect("the bounded read")
        .split("\n/// ")
        .next()
        .expect("it ends");

    assert!(
        bounded.find("metadata(").expect("a size check")
            < bounded.find("fs::read(").expect("a read"),
        "the size must be checked before the file is read"
    );
    assert_eq!(
        bounded.matches("MAX_SUBMISSION_BYTES").count(),
        4,
        "both the reported size and the read result must be checked"
    );

    // And nothing else in the shell reads a file without that helper.
    for file in sources(&root.join("src-tauri/src"), &["rs"]) {
        let contents = shipped_code(&file, &read(&file));
        if file
            .display()
            .to_string()
            .replace('\\', "/")
            .ends_with("remote_import.rs")
        {
            continue;
        }
        assert!(
            !contents.contains("fs::read"),
            "{} reads a file directly: use the bounded reader",
            file.display()
        );
    }
}

#[test]
fn the_import_adds_no_capability_and_no_plugin() {
    // ADR-0022 decision 17. Drag-drop needs neither: `core:default` already
    // grants `core:event:default`, and a Rust-side handler needs no permission.
    let root = repository_root();

    let manifest = read(&root.join("src-tauri/Cargo.toml"));
    assert!(
        !manifest.contains("tauri-plugin"),
        "no filesystem or dialog plugin may be added for import"
    );

    let capability = read(&root.join("src-tauri/capabilities/default.json"));
    for forbidden in [
        "fs:", "dialog:", "shell:", "http:", "process:", "os:", "sql:", "updater:",
    ] {
        assert!(
            !capability.contains(forbidden),
            "the default capability must not grant {forbidden}"
        );
    }
}

#[test]
fn the_generated_artifact_is_held_to_the_offline_contract() {
    // The npm guard reads the *template*'s built bytes at build time. Nothing
    // else reads a *generated* file, so `app-remote` carries its own list - and
    // this asserts that list has not quietly shrunk (ADR-0021 decision 1).
    let root = repository_root();
    let generate = read(&root.join("crates/app-remote/src/generate.rs"));

    for required in [
        "fetch(",
        "XMLHttpRequest",
        "WebSocket",
        "sendBeacon",
        "EventSource",
        "Bearer ",
        "session_token",
        "join_token",
        "token_hash",
    ] {
        assert!(
            generate.contains(&format!("\"{required}\"")),
            "the generated-artifact guard no longer refuses `{required}`"
        );
    }

    // Generation must verify before it returns, so a bad artefact is never
    // written to disk.
    assert!(
        generate.contains("verify_artifact(&artifact)?"),
        "generate() must verify its own output before returning it"
    );

    // The offline build guard itself is untouched and still literal.
    let offline = read(&root.join("scripts/check-remote-form-offline.mjs"));
    for required in [
        "external script",
        "external stylesheet",
        "absolute http(s) URL",
        "fetch() call",
        "XMLHttpRequest",
        "WebSocket",
        "navigator.sendBeacon",
        "EventSource",
    ] {
        assert!(
            offline.contains(required),
            "the offline build guard no longer checks for `{required}`"
        );
    }
}

#[test]
fn the_offline_form_carries_no_credential_and_calls_nothing() {
    // The bundle that ends up in a participant's browser with no network at
    // all. `no_bundle_assigns_html_directly` already covers the HTML sink; this
    // is about what it could reach for (architecture rules section 20).
    let root = repository_root();

    for file in sources(&root.join("apps/remote-form/src"), &["ts"]) {
        let contents = code_only(&read(&file));

        for forbidden in [
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "EventSource",
            "sendBeacon",
            "http://",
            "https://",
            // No credential reaches this bundle, because none is ever put in
            // the artefact it reads (ADR-0021 decision 3).
            "session_token",
            "join_token",
            "token_hash",
            "Bearer",
            "Authorization",
            // A submission leaves the browser only when the participant asks.
            "submit(",
            "action=",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: the remote form is offline and sends nothing",
                file.display()
            );
        }
    }

    // The injection point is one element, and the bundle reads it as text.
    let context = read(&root.join("apps/remote-form/src/context.ts"));
    assert!(
        context.contains("textContent") && context.contains("JSON.parse"),
        "the context must be read from textContent and parsed, never assigned as HTML"
    );
}

#[test]
fn the_remote_form_declares_only_workspace_dependencies() {
    // ADR-0009: this bundle carries no external dependency at all. The two it
    // declares are workspace packages that are themselves under the same guard.
    let root = repository_root();
    let manifest = read(&root.join("apps/remote-form/package.json"));

    let dependencies = manifest
        .split("\"dependencies\"")
        .nth(1)
        .expect("a dependencies block")
        .split('}')
        .next()
        .expect("the block ends");

    // One line per entry, each of which opens with the quoted package name.
    // The block's own `: {` is not an entry.
    let entries: Vec<&str> = dependencies
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('"'))
        .collect();

    assert!(!entries.is_empty(), "no dependencies were found to check");

    for entry in entries {
        assert!(
            entry.starts_with("\"@lan-meeting/"),
            "the remote form declared a non-workspace dependency: {entry}"
        );
    }

    for framework in ["react", "vue", "svelte", "preact", "solid-js"] {
        assert!(
            !dependencies.contains(framework),
            "the remote form must carry no UI framework (ADR-0009): {framework}"
        );
    }
}

#[test]
fn the_host_never_chooses_where_a_generated_form_goes() {
    // ADR-0021 decision 10. The directory comes from Tauri's application-data
    // path, resolved in Rust; the window names a meeting and a participant.
    let root = repository_root();

    let gateway = read(&root.join("apps/host-ui/src/api/hostApi.ts"));
    let call = gateway
        .split("'generate_remote_form'")
        .nth(1)
        .expect("the generation call")
        .split("}")
        .next()
        .expect("the argument object ends");
    for forbidden in ["path", "directory", "folder", "file_name"] {
        assert!(
            !call.contains(forbidden),
            "the window passed `{forbidden}` to generate_remote_form: the backend chooses the path"
        );
    }

    let commands = read(&root.join("src-tauri/src/commands.rs"));
    assert!(
        commands.contains("app_data_dir()"),
        "the generation command must resolve the application data directory itself"
    );
}

// ---------------------------------------------------------------------------
// Scope: the LAN is the only network
// ---------------------------------------------------------------------------

#[test]
fn no_relay_or_outbound_network_code_has_crept_in() {
    // WebSocket left this list in step 7, when it became the participant
    // notification transport. Nothing else did.
    //
    // What remains is the "no cloud, no relay, no tunnel, no public hosting"
    // promise (PRD section 4). These are outbound-client, hole-punching and
    // relay names: the absence of a remote *origin* is enforced separately, by
    // the content security policy the server sends and the tests over it.
    //
    // `tokio_tungstenite` stays forbidden in shipped code even though the crate
    // is now in the tree: it is a WebSocket *client*, this application has
    // nothing legitimate to be a client of, and the real-socket tests are the
    // only place it belongs.
    let root = repository_root();
    let mut files = sources(&root.join("src-tauri/src"), &["rs"]);
    files.extend(sources(&root.join("crates/app-server/src"), &["rs"]));
    files.extend(sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]));
    files.extend(sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]));

    for file in files {
        let contents = code_only(&read(&file));
        for forbidden in [
            // Outbound HTTP and WebSocket clients.
            "reqwest",
            "ureq",
            "hyper::Client",
            "tokio_tungstenite",
            // A second, unmanaged push channel.
            "EventSource",
            // Polling in a UI that is now event-driven. A reconnect uses a
            // one-shot `setTimeout`; a repeating timer is how a bundle starts
            // hammering the Host machine.
            "setInterval",
            // Relays, tunnels and hole punching. The join URL is reachable on
            // the local network or it is not reachable at all.
            "ngrok",
            "localtunnel",
            "upnp",
            "UPnP",
            "RTCPeerConnection",
            "webrtc",
            "stun:",
            "turn:",
            "relayServer",
            "public_gateway",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: this application has no cloud, relay or tunnel",
                file.display()
            );
        }
    }
}

#[test]
fn the_notification_socket_never_carries_a_credential_in_its_url() {
    // The whole reason the credential travels in `Sec-WebSocket-Protocol`
    // (ADR-0018). A token in a query string reaches access logs, `Referer`
    // headers and browser history, and this is the guard against somebody
    // finding that easier one day.
    let root = repository_root();
    let ws = code_only(&read(&root.join("crates/app-server/src/ws.rs")));

    // No URL-derived extractor on this route: there is nothing in the path or
    // the query for the handler to read, so identity cannot come from there.
    for forbidden in ["Query", "Path<", "RawQuery", "OriginalUri"] {
        assert!(
            !ws.contains(forbidden),
            "the socket route must take no identity from the URL, found `{forbidden}`"
        );
    }
    assert!(
        ws.contains("SEC_WEBSOCKET_PROTOCOL"),
        "the credential is read from the subprotocol header"
    );

    // And the client side builds the URL without one.
    let client = code_only(&read(&root.join("apps/lan-ui/src/api/realtime.ts")));
    for forbidden in ["?token", "&token", "token=", "#token"] {
        assert!(
            !client.contains(forbidden),
            "the participant bundle must not put a credential in the socket URL: `{forbidden}`"
        );
    }
}

#[test]
fn only_one_module_in_the_participant_bundle_opens_a_socket() {
    // The same rule `fetch` and browser storage already have: one module owns
    // the credential's use, so "where does the token go" has one answer.
    let root = repository_root();
    let keeper = root.join("apps/lan-ui/src/api/realtime.ts");

    let mut openers = Vec::new();
    for file in sources(&root.join("apps/lan-ui/src"), &["ts", "tsx"]) {
        if code_only(&read(&file)).contains("new WebSocket") {
            openers.push(file);
        }
    }

    assert_eq!(
        openers,
        vec![keeper],
        "only apps/lan-ui/src/api/realtime.ts may open a WebSocket"
    );
}

#[test]
fn the_host_ui_has_no_socket_of_its_own() {
    // The Host is in the same process as the database and hears domain events
    // over Tauri IPC. A socket to loopback would route the Host own view
    // through the transport that exists to face untrusted input, and would
    // stop working whenever the Host chose not to start that server
    // (ADR-0018).
    let root = repository_root();
    for file in sources(&root.join("apps/host-ui/src"), &["ts", "tsx"]) {
        let contents = code_only(&read(&file));
        for forbidden in ["WebSocket", "ws://", "wss://"] {
            assert!(
                !contents.contains(forbidden),
                "{} mentions `{forbidden}`: the Host listens over Tauri IPC",
                file.display()
            );
        }
    }
}

#[test]
fn presence_is_never_written_to_the_audit_log() {
    // Architecture rules section 17: the audit log records what people did to
    // the meeting. A socket opening is an observation, and burying the real
    // entries under connection noise would make the log unreadable (ADR-0018).
    let root = repository_root();
    let audit = read(&root.join("crates/app-core/src/audit.rs"));

    for forbidden in ["Presence", "Connected", "Disconnected", "SocketOpened"] {
        assert!(
            !audit.contains(forbidden),
            "the audit vocabulary must not gain `{forbidden}`"
        );
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
