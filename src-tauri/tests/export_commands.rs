//! Export generation, against a real SQLite database and a real filesystem.
//!
//! These call [`HostState`] directly, for the reason `host_commands.rs` and
//! `remote_commands.rs` give: the `#[tauri::command]` function is a one-line
//! delegation to exactly this method, so what is under test is the whole
//! adapter - the read-only eligibility gate, the pure render, the file write
//! and the audit-recording transaction (Step 12).
//!
//! The output directory is a parameter rather than something the state
//! holds, exactly as `remote_commands.rs` already does it: in the running
//! application the command layer resolves it from Tauri's application-data
//! directory, and the window never sees or chooses it.

use std::path::PathBuf;

use lan_meeting_app_lib::dto::{MeetingConfigurationInput, ParticipantDetailsInput};
use lan_meeting_app_lib::error::{ErrorCategory, HostErrorKind};
use lan_meeting_app_lib::HostState;
use tempfile::TempDir;

struct Host {
    state: HostState,
    dir: TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let state = HostState::open(dir.path().join("meetings.sqlite3")).expect("open");
        Host { state, dir }
    }

    fn output(&self) -> PathBuf {
        self.dir.path().join("exports")
    }

    fn draft(&self) -> String {
        self.state
            .create_meeting(configuration())
            .expect("create")
            .meeting_id
            .to_storage()
    }

    /// An opened meeting with one participant on the roster, who has not
    /// written a note.
    fn open(&self) -> (String, String) {
        let meeting_id = self.draft();
        let participant_id = self
            .state
            .add_participant(&meeting_id, details("Budi Santoso"))
            .expect("add")
            .participant_id
            .to_storage();
        self.state.open_meeting(&meeting_id).expect("open");
        (meeting_id, participant_id)
    }

    /// An opened meeting whose roster is exactly `names`, added in the given
    /// order while still `DRAFT` (the roster is only ever settled before a
    /// meeting opens - ADR-0013).
    fn open_with_roster(&self, names: &[&str]) -> (String, Vec<String>) {
        let meeting_id = self.draft();
        let participant_ids = names
            .iter()
            .map(|name| {
                self.state
                    .add_participant(&meeting_id, details(name))
                    .expect("add")
                    .participant_id
                    .to_storage()
            })
            .collect();
        self.state.open_meeting(&meeting_id).expect("open");
        (meeting_id, participant_ids)
    }

    fn export_file_count(&self) -> usize {
        match std::fs::read_dir(self.output()) {
            Ok(entries) => entries.count(),
            Err(_) => 0, // The directory itself is never created for a refused export.
        }
    }

    fn exported_audit_rows(&self, meeting_id: &str) -> usize {
        self.state
            .list_audit_entries(meeting_id)
            .expect("audit entries")
            .iter()
            .filter(|entry| entry.action.as_str() == "meeting.exported")
            .count()
    }
}

fn configuration() -> MeetingConfigurationInput {
    MeetingConfigurationInput {
        title: "Weekly Coordination".to_owned(),
        topic: None,
        date: "2026-09-20".to_owned(),
        start_time: "09:00:00".to_owned(),
        end_time: "10:30:00".to_owned(),
        timezone: "Asia/Makassar".to_owned(),
        location: None,
        description: None,
    }
}

fn details(name: &str) -> ParticipantDetailsInput {
    ParticipantDetailsInput {
        name: name.to_owned(),
        department: None,
        position: None,
        meeting_role: None,
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[test]
fn a_draft_meeting_reaches_neither_the_renderer_nor_the_filesystem() {
    let host = Host::new();
    let meeting_id = host.draft();

    let error = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap_err();

    assert_eq!(error.category, ErrorCategory::Lifecycle);
    assert!(matches!(error.kind, HostErrorKind::MeetingNotOpen { .. }));

    // Not "returned an error" alone: nothing was written at all.
    assert_eq!(host.export_file_count(), 0, "no file must exist");
    assert_eq!(
        host.exported_audit_rows(&meeting_id),
        0,
        "no audit row must exist"
    );
}

#[test]
fn an_open_meeting_exports_successfully() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    let result = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();

    assert_eq!(result.meeting_id.to_storage(), meeting_id);
    assert!(result.bytes > 0);
    assert!(std::path::Path::new(&result.path).is_file());
    assert_eq!(host.exported_audit_rows(&meeting_id), 1);
}

#[test]
fn a_locked_meeting_exports_successfully() {
    let host = Host::new();
    let (meeting_id, _) = host.open();
    host.state.lock_meeting(&meeting_id).unwrap();

    let result = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();

    assert!(std::path::Path::new(&result.path).is_file());
    assert_eq!(host.exported_audit_rows(&meeting_id), 1);
}

#[test]
fn repeated_exports_each_succeed_and_record_a_separate_audit_row() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    let first = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();
    let second = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();

    assert_ne!(first.path, second.path, "each export is its own file");
    assert!(
        std::path::Path::new(&first.path).is_file(),
        "the first export is not overwritten"
    );
    assert!(std::path::Path::new(&second.path).is_file());
    assert_eq!(host.exported_audit_rows(&meeting_id), 2);
}

#[test]
fn an_unknown_meeting_is_reported_as_not_found() {
    let host = Host::new();
    let ghost = app_core::id::MeetingId::new().to_storage();

    let error = host
        .state
        .generate_export(&ghost, "markdown", &host.output())
        .unwrap_err();

    assert_eq!(error.category, ErrorCategory::NotFound);
    assert!(matches!(error.kind, HostErrorKind::MeetingNotFound { .. }));
    assert_eq!(host.export_file_count(), 0);
}

#[test]
fn a_malformed_meeting_id_is_refused_before_anything_is_read() {
    let host = Host::new();

    let error = host
        .state
        .generate_export("not-a-uuid", "markdown", &host.output())
        .unwrap_err();

    assert_eq!(error.category, ErrorCategory::Validation);
    assert_eq!(host.export_file_count(), 0);
}

#[test]
fn an_unknown_format_is_refused_as_a_validation_error() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    let error = host
        .state
        .generate_export(&meeting_id, "pdf", &host.output())
        .unwrap_err();

    assert_eq!(error.category, ErrorCategory::Validation);
    assert_eq!(host.export_file_count(), 0);
    assert_eq!(host.exported_audit_rows(&meeting_id), 0);
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn identical_source_state_produces_byte_identical_markdown() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    host.state
        .write_participant_note(
            &meeting_id,
            &participant_id,
            "## Decisions\n\n- Approve the budget",
        )
        .unwrap();
    host.state.lock_meeting(&meeting_id).unwrap();

    let first = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();
    let second = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();

    let first_bytes = std::fs::read(&first.path).unwrap();
    let second_bytes = std::fs::read(&second.path).unwrap();
    assert_eq!(
        first_bytes, second_bytes,
        "two exports of an unchanged, LOCKED meeting must be byte-identical"
    );
}

#[test]
fn generated_at_never_appears_inside_the_file_content() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    let result = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();

    let content = std::fs::read_to_string(&result.path).unwrap();
    let stamp = result.generated_at.to_string();
    assert!(
        !content.contains(&stamp),
        "the DTO's generated_at must not appear in the file"
    );
}

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

#[test]
fn a_participant_without_a_note_is_shown_as_such_in_every_format() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    for format in ["markdown", "txt", "ai_context"] {
        let result = host
            .state
            .generate_export(&meeting_id, format, &host.output())
            .unwrap();
        let content = std::fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("No note written"), "{format}: {content}");
        assert!(content.contains("Budi Santoso"), "{format}");
    }
}

#[test]
fn markdown_preserves_a_notes_own_heading_verbatim_without_relevelling() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    host.state
        .write_participant_note(&meeting_id, &participant_id, "# Important Decision\n\ntext")
        .unwrap();

    let result = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();
    let content = std::fs::read_to_string(&result.path).unwrap();

    assert!(
        content.contains("# Important Decision"),
        "the note's own heading must survive at its original level: {content}"
    );
    assert!(
        !content.contains("#### Important Decision"),
        "the heading must not be re-levelled"
    );
}

#[test]
fn ai_context_injects_no_semantic_label_and_preserves_author_headings() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    host.state
        .write_participant_note(
            &meeting_id,
            &participant_id,
            "## Decisions\n\nApprove the budget.",
        )
        .unwrap();

    let result = host
        .state
        .generate_export(&meeting_id, "ai_context", &host.output())
        .unwrap();
    let content = std::fs::read_to_string(&result.path).unwrap();

    assert!(
        content.starts_with(
            "The following is a structured export of a meeting record. It has been formatted only; nothing has been summarized, inferred, or added."
        ),
        "{content}"
    );
    assert!(
        content.contains("Decisions"),
        "author heading survives: {content}"
    );
    assert!(content.contains("Approve the budget."));
    // No injected label distinct from what the author wrote, and no other
    // preamble sentence naming the meeting's content specifically.
    assert!(!content.contains("Action Item"));
}

// ---------------------------------------------------------------------------
// Authorization
// ---------------------------------------------------------------------------

#[test]
fn a_remote_import_actor_cannot_generate_an_export() {
    let host = Host::new();
    let (meeting_id, _) = host.open();
    let parsed = app_core::id::MeetingId::parse(&meeting_id).unwrap();

    let error = host
        .state
        .domain()
        .record_export(
            &app_core::actor::Actor::RemoteImport {
                meeting_id: parsed,
                participant_id: app_core::id::ParticipantId::new(),
            },
            parsed,
            app_core::service::ExportFormat::Markdown,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        app_core::error::DomainError::Forbidden {
            actor_type: "REMOTE_IMPORT",
            ..
        }
    ));
    assert_eq!(host.exported_audit_rows(&meeting_id), 0);
}

// ---------------------------------------------------------------------------
// Lifecycle interleaving (frozen design: two independent checks)
// ---------------------------------------------------------------------------

/// Proves the two-check design's actual claim: a meeting the eligibility
/// check found `OPEN` can transition to `LOCKED` before the second,
/// authoritative check runs, and that second check must still succeed -
/// because `Domain::record_export` (via `Meeting::ensure_exportable`)
/// accepts `LOCKED` exactly as it accepts `OPEN`.
///
/// `HostState::check_export_eligible` is a private method, so this test
/// cannot call it directly without adding a test-only seam to production
/// code, which is out of scope for this pass (per instruction: do not modify
/// production code merely to make a test easier). It instead reconstructs
/// the exact two checks that method performs, using only public production
/// APIs:
///
/// - the meeting-status half, via the real `HostState::get_meeting` (the
///   same `HostQueries` read path `check_export_eligible` itself uses);
/// - the authorization half, via the real, public `app_core::authz::
///   authorize` free function `check_export_eligible` calls verbatim.
///
/// Between that reconstructed "stage 1" and the real "stage 2"
/// (`Domain::record_export`, called through the public `HostState::domain()`
/// exactly as the `RemoteImport` authorization test above already does), the
/// meeting is locked through the real command path
/// (`HostState::lock_meeting`) - the one legal transition the lifecycle
/// permits between the two stages. This is not a fake lifecycle transition:
/// it is the real, only transition `OPEN` can ever make.
#[test]
fn a_meeting_locked_between_the_two_checks_still_exports() {
    let host = Host::new();
    let (meeting_id, _) = host.open();
    let parsed = app_core::id::MeetingId::parse(&meeting_id).unwrap();

    // Stage 1, reconstructed: the meeting is found, OPEN, and the Host is
    // authorized for `GenerateExport` - exactly what `check_export_eligible`
    // itself checks, using the same public primitives it uses internally.
    let detail_before = host.state.get_meeting(&meeting_id).unwrap();
    assert_eq!(detail_before.status.as_str(), "OPEN");
    app_core::authz::authorize(
        &app_core::actor::Actor::Host,
        parsed,
        app_core::authz::Operation::GenerateExport,
    )
    .expect("the real authorize() must accept Host for GenerateExport while OPEN");

    // The meeting changes state between the two checks.
    host.state.lock_meeting(&meeting_id).unwrap();

    // Stage 2: the real, authoritative check must still succeed against the
    // now-`LOCKED` meeting.
    let recorded = host
        .state
        .domain()
        .record_export(
            &app_core::actor::Actor::Host,
            parsed,
            app_core::service::ExportFormat::Markdown,
        )
        .expect("record_export must still succeed against the now-LOCKED meeting");

    assert_eq!(recorded.meeting_id, parsed);
    assert_eq!(recorded.format, app_core::service::ExportFormat::Markdown);
    assert_eq!(host.exported_audit_rows(&meeting_id), 1);
}

// ---------------------------------------------------------------------------
// Filesystem write failure
// ---------------------------------------------------------------------------

/// The same technique `remote_commands.rs::an_unwritable_directory_is_
/// reported_as_the_hosts_own_problem` already uses: a regular file sits
/// where the output directory should be, so `std::fs::create_dir_all` cannot
/// succeed. Eligibility and rendering both complete first (rendering is
/// pure, in-memory, and happens before any filesystem call), so this
/// specifically exercises the filesystem-write failure path, distinct from
/// every lifecycle/authorization refusal tested elsewhere in this file.
#[test]
fn an_unwritable_directory_is_reported_as_the_hosts_own_problem() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    // A file where the directory should be.
    let blocked = host.dir.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("write the blocker");

    let error = host
        .state
        .generate_export(&meeting_id, "markdown", &blocked)
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::Persistence),
        "{error:?}"
    );
    assert_eq!(error.category, ErrorCategory::Unexpected);
    // The Host is told where, because it is their own machine.
    assert!(error.message.contains("blocked"), "{}", error.message);

    // Nothing was recorded: the write never happened, so there is nothing to
    // record an export of.
    assert_eq!(host.exported_audit_rows(&meeting_id), 0);
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

/// Roster order in the export must be `name ASC, id ASC` - the same order
/// `HostQueries::participants` already guarantees and every other Host UI
/// roster view already relies on - and never insertion order. Participant
/// ids themselves must never appear in the rendered document (E-8).
#[test]
fn participants_are_exported_in_name_then_id_order_and_ids_are_never_rendered() {
    let host = Host::new();
    // Inserted deliberately out of alphabetical order.
    let (meeting_id, participant_ids) =
        host.open_with_roster(&["Siti Rahayu", "Ahmad Fauzi", "Budi Santoso"]);

    let result = host
        .state
        .generate_export(&meeting_id, "markdown", &host.output())
        .unwrap();
    let content = std::fs::read_to_string(&result.path).unwrap();

    let position = |needle: &str| {
        content
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} not found in:\n{content}"))
    };
    let ahmad = position("Ahmad Fauzi");
    let budi = position("Budi Santoso");
    let siti = position("Siti Rahayu");

    assert!(
        ahmad < budi && budi < siti,
        "participants must appear in name order (Ahmad, Budi, Siti), got:\n{content}"
    );

    for id in &participant_ids {
        assert!(
            !content.contains(id.as_str()),
            "participant id {id} must never appear in the rendered document"
        );
    }
}
