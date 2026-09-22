//! The Host's note commands, against a real SQLite database.
//!
//! These call [`HostState`] directly, for the reason `host_commands.rs` gives:
//! the `#[tauri::command]` functions are one-line delegations to exactly these
//! methods, so this is the whole adapter under test - parsing, the `Actor::Host`
//! mint, the domain call, the query call and the error mapping.
//!
//! What they assert is that the adapter adds no rules of its own. Every refusal
//! below is a *domain* refusal arriving intact.

use app_core::note::MAX_NOTE_BYTES;
use lan_meeting_app_lib::dto::{MeetingConfigurationInput, ParticipantDetailsInput};
use lan_meeting_app_lib::error::{ErrorCategory, HostErrorKind};
use lan_meeting_app_lib::HostState;
use tempfile::TempDir;

struct Host {
    state: HostState,
    _dir: TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let state = HostState::open(dir.path().join("meetings.sqlite3")).expect("open");
        Host { state, _dir: dir }
    }

    fn draft(&self) -> String {
        self.state
            .create_meeting(configuration())
            .expect("create")
            .meeting_id
            .to_storage()
    }

    /// A meeting that has been opened, which is where notes normally happen.
    fn open(&self) -> String {
        let meeting_id = self.draft();
        self.state
            .add_participant(&meeting_id, details("Budi Santoso"))
            .expect("add");
        self.state.open_meeting(&meeting_id).expect("open");
        meeting_id
    }

    fn add(&self, meeting_id: &str, name: &str) -> String {
        self.state
            .add_participant(meeting_id, details(name))
            .expect("add")
            .participant_id
            .to_storage()
    }

    fn first_participant(&self, meeting_id: &str) -> String {
        self.state.list_participants(meeting_id).expect("roster")[0]
            .id
            .to_storage()
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
// Writing
// ---------------------------------------------------------------------------

#[test]
fn the_host_can_create_and_then_replace_a_participants_note() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    let created = host
        .state
        .write_participant_note(&meeting_id, &participant_id, "## Agenda\n\nBudget.")
        .expect("write");
    assert!(created.created);
    assert_eq!(created.version, 1);

    let updated = host
        .state
        .write_participant_note(&meeting_id, &participant_id, "## Agenda\n\nVenue too.")
        .expect("write again");
    assert!(!updated.created);
    assert_eq!(updated.version, 2);
    // One note per participant, whatever the version (ADR-0003).
    assert_eq!(updated.note_id, created.note_id);
}

#[test]
fn a_host_note_is_attributed_to_the_host_and_to_no_participant() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);
    host.state
        .write_participant_note(&meeting_id, &participant_id, "written by the host")
        .expect("write");

    let note = host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .expect("a note");
    assert_eq!(note.last_author_type, "HOST");
    assert_eq!(note.last_author_id, None);
}

#[test]
fn notes_are_writable_in_draft_and_open_and_refused_when_locked() {
    // Decision D7: the existing behaviour is kept. Configuration and the
    // roster are settled in DRAFT; notes are the opposite case and stay
    // writable until the meeting is locked.
    let host = Host::new();
    let meeting_id = host.draft();
    let participant_id = host.add(&meeting_id, "Budi Santoso");

    assert!(host
        .state
        .write_participant_note(&meeting_id, &participant_id, "while drafting")
        .is_ok());

    host.state.open_meeting(&meeting_id).expect("open");
    assert!(host
        .state
        .write_participant_note(&meeting_id, &participant_id, "while open")
        .is_ok());
}

#[test]
fn a_locked_meeting_refuses_a_note_write() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);
    host.state
        .write_participant_note(&meeting_id, &participant_id, "before the lock")
        .expect("write");

    // Locking is not exposed as a command, so the domain is used directly -
    // which is also the point: the refusal below is the domain's, re-read
    // inside the write's own transaction.
    host.state
        .domain()
        .lock_meeting(
            &app_core::actor::Actor::Host,
            app_core::id::MeetingId::parse(&meeting_id).expect("id"),
        )
        .expect("lock");

    let refused = host
        .state
        .write_participant_note(&meeting_id, &participant_id, "after the lock")
        .expect_err("a locked meeting refuses writes");
    assert!(matches!(refused.kind, HostErrorKind::MeetingLocked { .. }));
    assert_eq!(refused.category, ErrorCategory::Lifecycle);

    // And the note is untouched.
    assert_eq!(
        host.state
            .get_participant_note(&meeting_id, &participant_id)
            .expect("read")
            .expect("a note")
            .content,
        "before the lock"
    );
}

#[test]
fn a_participant_outside_the_meeting_is_refused() {
    let host = Host::new();
    let meeting_id = host.open();
    let elsewhere = host.open();
    let theirs = host.first_participant(&elsewhere);

    let refused = host
        .state
        .write_participant_note(&meeting_id, &theirs, "not yours")
        .expect_err("refused");
    assert!(matches!(
        refused.kind,
        HostErrorKind::ParticipantNotFound { .. }
    ));
}

#[test]
fn a_malformed_identifier_is_a_validation_refusal() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    for (meeting, participant) in [
        ("not-an-id", participant_id.as_str()),
        (meeting_id.as_str(), "not-an-id"),
    ] {
        let refused = host
            .state
            .write_participant_note(meeting, participant, "x")
            .expect_err("refused");
        assert_eq!(refused.category, ErrorCategory::Validation);
    }
}

// ---------------------------------------------------------------------------
// Content rules arrive from the domain, intact
// ---------------------------------------------------------------------------

#[test]
fn content_the_editor_would_refuse_is_refused_here_too() {
    // The browser copy of these rules is a convenience. This is the copy that
    // decides whether content is stored, and it runs even for a caller that
    // never went near the editor.
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    for bad in [
        "",
        "   ",
        "<script>alert(1)</script>",
        "</div>",
        "<!-- comment -->",
        "<?xml version=\"1.0\"?>",
        "[click](javascript:alert(1))",
        "[click](data:text/html,x)",
        "[notes](../notes.md)",
        "before\u{0}after",
        "before\u{1b}after",
    ] {
        let refused = host
            .state
            .write_participant_note(&meeting_id, &participant_id, bad)
            .expect_err(&format!("{bad:?} should have been refused"));
        assert_eq!(refused.category, ErrorCategory::Validation, "{bad:?}");
    }

    // And nothing was stored by any of them.
    assert!(host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .is_none());
}

#[test]
fn ordinary_arithmetic_is_not_refused_as_html() {
    // The regression the raw-HTML rule exists for. A Host writing `a < b` in a
    // meeting note must not be told their note contains HTML.
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    for good in [
        "a < b",
        "2 < 3",
        "throughput 2 < 3 > 1 across the day",
        "budget a<b was measured",
        "`<script>` written in a code span",
        "```\n<script>alert(1)</script>\n```",
    ] {
        host.state
            .write_participant_note(&meeting_id, &participant_id, good)
            .unwrap_or_else(|e| panic!("{good:?} should be accepted: {e:?}"));
    }
}

#[test]
fn the_size_limit_is_enforced_by_the_backend() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    host.state
        .write_participant_note(&meeting_id, &participant_id, &"a".repeat(MAX_NOTE_BYTES))
        .expect("exactly the limit is accepted");

    let refused = host
        .state
        .write_participant_note(
            &meeting_id,
            &participant_id,
            &"a".repeat(MAX_NOTE_BYTES + 1),
        )
        .expect_err("one byte more is refused");
    assert_eq!(refused.category, ErrorCategory::Validation);
    assert!(refused.message.contains("65536"), "{}", refused.message);
}

#[test]
fn windows_line_endings_are_normalised_at_the_transport() {
    // A WebView on Windows can submit CRLF, and the domain refuses a carriage
    // return as a control character. Normalising here keeps a Host from being
    // told about a character they cannot see, and keeps stored content uniform.
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    host.state
        .write_participant_note(&meeting_id, &participant_id, "line one\r\nline two\r\n")
        .expect("CRLF is accepted");

    let note = host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .expect("a note");
    assert_eq!(note.content, "line one\nline two\n");
    assert!(!note.content.contains('\r'));
}

#[test]
fn nothing_else_about_the_content_is_normalised() {
    // The stored note is what was typed. No reformatting, no re-serialising,
    // no tidying of spacing or list markers (ADR-0019).
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    let typed = "#  Odd   spacing\n\n\n\n-    loose    marker\n\n   trailing indent";
    host.state
        .write_participant_note(&meeting_id, &participant_id, typed)
        .expect("write");

    assert_eq!(
        host.state
            .get_participant_note(&meeting_id, &participant_id)
            .expect("read")
            .expect("a note")
            .content,
        typed
    );
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn a_participant_without_a_note_reads_as_null() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    assert!(host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .is_none());
    assert!(host
        .state
        .list_note_versions(&meeting_id, &participant_id)
        .expect("read")
        .is_empty());
}

#[test]
fn history_is_newest_first_and_carries_no_body() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    for round in 1..=3 {
        host.state
            .write_participant_note(&meeting_id, &participant_id, &format!("round {round}"))
            .expect("write");
    }

    let versions = host
        .state
        .list_note_versions(&meeting_id, &participant_id)
        .expect("read");
    assert_eq!(
        versions.iter().map(|v| v.version).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
    assert_eq!(versions[0].byte_length, "round 3".len() as i64);

    // The body of one version is fetched on its own.
    let earlier = host
        .state
        .get_note_version(&meeting_id, &participant_id, 1)
        .expect("read")
        .expect("version 1");
    assert_eq!(earlier.content, "round 1");
}

#[test]
fn a_version_that_does_not_exist_reads_as_null() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);
    host.state
        .write_participant_note(&meeting_id, &participant_id, "only one")
        .expect("write");

    for version in [0, 2, -5] {
        assert!(host
            .state
            .get_note_version(&meeting_id, &participant_id, version)
            .expect("read")
            .is_none());
    }
}

#[test]
fn history_survives_a_later_edit_unchanged() {
    // `note_versions` is append-only and the database refuses `UPDATE` on it.
    // What a reader should see is the old body, still the old body.
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    host.state
        .write_participant_note(&meeting_id, &participant_id, "the original")
        .expect("write");
    host.state
        .write_participant_note(&meeting_id, &participant_id, "the replacement")
        .expect("write");

    assert_eq!(
        host.state
            .get_note_version(&meeting_id, &participant_id, 1)
            .expect("read")
            .expect("version 1")
            .content,
        "the original"
    );
}

#[test]
fn the_overview_covers_the_whole_roster() {
    let host = Host::new();
    let meeting_id = host.draft();
    host.add(&meeting_id, "Alice Anwar");
    host.add(&meeting_id, "Bob Basuki");
    let alice = host.first_participant(&meeting_id);
    host.state.open_meeting(&meeting_id).expect("open");

    host.state
        .write_participant_note(&meeting_id, &alice, "written")
        .expect("write");

    let overview = host.state.list_notes_overview(&meeting_id).expect("read");
    assert_eq!(overview.len(), 2);
    assert_eq!(overview[0].name, "Alice Anwar");
    assert_eq!(overview[0].version, Some(1));
    assert_eq!(overview[1].name, "Bob Basuki");
    assert_eq!(overview[1].version, None);
    assert_eq!(overview[1].note_id, None);
}

#[test]
fn reading_a_note_is_unaffected_by_the_lock() {
    // A locked meeting is finished, not secret. Its notes are what the Host
    // locked it to keep.
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);
    host.state
        .write_participant_note(&meeting_id, &participant_id, "kept")
        .expect("write");

    host.state
        .domain()
        .lock_meeting(
            &app_core::actor::Actor::Host,
            app_core::id::MeetingId::parse(&meeting_id).expect("id"),
        )
        .expect("lock");

    assert!(host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .is_some());
    assert_eq!(
        host.state
            .list_note_versions(&meeting_id, &participant_id)
            .expect("read")
            .len(),
        1
    );
    assert!(host
        .state
        .get_note_version(&meeting_id, &participant_id, 1)
        .expect("read")
        .is_some());
}

// ---------------------------------------------------------------------------
// The audit trail
// ---------------------------------------------------------------------------

#[test]
fn a_note_write_is_audited_as_the_host_with_its_version() {
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    host.state
        .write_participant_note(&meeting_id, &participant_id, "first")
        .expect("write");
    host.state
        .write_participant_note(&meeting_id, &participant_id, "second")
        .expect("write");

    let entries = host.state.list_audit_entries(&meeting_id).expect("read");
    let note_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.action.starts_with("note."))
        .collect();

    assert_eq!(note_entries.len(), 2);
    assert_eq!(note_entries[0].action, "note.created");
    assert_eq!(note_entries[1].action, "note.updated");

    for entry in &note_entries {
        assert_eq!(entry.actor_type, "HOST");
        assert_eq!(entry.actor_id, None);
        assert_eq!(entry.target_type, "note");
    }

    // The metadata records the version, and never the note's content.
    let metadata = note_entries[1]
        .metadata
        .as_ref()
        .expect("metadata")
        .to_string();
    assert!(metadata.contains("\"version\":2"), "{metadata}");
    assert!(!metadata.contains("second"), "{metadata}");
}

#[test]
fn a_refused_write_leaves_no_note_no_version_and_no_audit_entry() {
    // The whole mutation is one transaction: a refusal rolls back the note,
    // its history row and its audit record together.
    let host = Host::new();
    let meeting_id = host.open();
    let participant_id = host.first_participant(&meeting_id);

    let before = host
        .state
        .list_audit_entries(&meeting_id)
        .expect("read")
        .len();

    assert!(host
        .state
        .write_participant_note(&meeting_id, &participant_id, "<script>alert(1)</script>")
        .is_err());

    assert!(host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .is_none());
    assert!(host
        .state
        .list_note_versions(&meeting_id, &participant_id)
        .expect("read")
        .is_empty());
    assert_eq!(
        host.state
            .list_audit_entries(&meeting_id)
            .expect("read")
            .len(),
        before
    );
}
