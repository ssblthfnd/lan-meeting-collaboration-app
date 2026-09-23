//! Remote submission import at the Host boundary, end to end.
//!
//! These call [`HostState`] directly, for the reason `host_commands.rs` gives:
//! the `#[tauri::command]` functions are thin delegations to exactly these
//! methods, so what is under test is the whole adapter - parsing, resolution,
//! the Host-selected-meeting rule, the actor's construction and the error
//! mapping.
//!
//! What the commands add on top is the *pending slot*, which is tested in
//! `src/remote_import.rs`: neither preview nor confirmation takes an artefact or
//! a path, so the window cannot substitute one.
//!
//! Every submission here is a real JSON artefact of the frozen v1 contract, and
//! the hash the domain receives is computed by `app-remote` - the one
//! implementation there is. That makes these the tests that prove hash reuse end
//! to end.

use app_core::actor::Actor;
use app_core::id::{MeetingId, ParticipantId, SubmissionId};
use lan_meeting_app_lib::dto::{
    ImportBlockerDto, MeetingConfigurationInput, ParticipantDetailsInput,
};
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

    fn add(&self, meeting_id: &str, name: &str) -> String {
        self.state
            .add_participant(meeting_id, details(name))
            .expect("add")
            .participant_id
            .to_storage()
    }

    /// An open meeting with one participant.
    fn open(&self) -> (String, String) {
        let meeting_id = self.draft();
        let participant_id = self.add(&meeting_id, "Budi Santoso");
        self.state.open_meeting(&meeting_id).expect("open");
        (meeting_id, participant_id)
    }

    /// An open meeting with two, for the cross-participant cases. Both are
    /// added before the meeting opens: the roster settles in `DRAFT`.
    fn open_two(&self) -> (String, String, String) {
        let meeting_id = self.draft();
        let budi = self.add(&meeting_id, "Budi Santoso");
        let siti = self.add(&meeting_id, "Siti Rahayu");
        self.state.open_meeting(&meeting_id).expect("open");
        (meeting_id, budi, siti)
    }

    /// Row counts for everything an import touches.
    fn counts(&self) -> (i64, i64, i64, i64) {
        let db = self.state.shared_db();
        db.read(|conn| {
            let one =
                |sql: &str| -> i64 { conn.query_row(sql, [], |row| row.get(0)).expect("count") };
            Ok((
                one("SELECT count(*) FROM notes"),
                one("SELECT count(*) FROM note_versions"),
                one("SELECT count(*) FROM remote_submissions"),
                one("SELECT count(*) FROM audit_logs"),
            ))
        })
        .expect("read")
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

/// A submission file, exactly as the offline form writes one.
struct Artifact {
    submission_id: String,
    json: String,
}

fn artifact(meeting_id: &str, participant_id: &str, note: &str) -> Artifact {
    named(meeting_id, participant_id, note, "Budi Santoso", 0)
}

fn named(
    meeting_id: &str,
    participant_id: &str,
    note: &str,
    participant_name: &str,
    source_version: i64,
) -> Artifact {
    let submission_id = SubmissionId::new().to_storage();
    let json = serde_json::json!({
        "schema_version": 1,
        "submission_id": submission_id,
        "meeting_id": meeting_id,
        "participant_id": participant_id,
        "participant_name": participant_name,
        "source_version": source_version,
        "generated_at": "2026-09-23T01:00:00.000Z",
        "submitted_at": "2026-09-23T04:12:07.000Z",
        "note": note,
    })
    .to_string();

    Artifact {
        submission_id,
        json,
    }
}

/// The same artefact re-exported with different content: same id, new note.
fn re_exported(original: &Artifact, note: &str) -> Artifact {
    let mut value: serde_json::Value = serde_json::from_str(&original.json).expect("json");
    value["note"] = serde_json::json!(note);
    Artifact {
        submission_id: original.submission_id.clone(),
        json: value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

#[test]
fn a_preview_reports_the_meeting_the_participant_and_the_note() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## From offline");

    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "submission.json")
        .expect("previews");

    assert_eq!(preview.origin, "submission.json");
    assert_eq!(preview.meeting_title, "Weekly Coordination");
    assert!(preview.meeting_matches);
    assert_eq!(preview.participant_name.as_deref(), Some("Budi Santoso"));
    assert!(preview.participant_name_matches);
    assert_eq!(preview.submitted_note, "## From offline");
    assert_eq!(preview.current_note, None);
    assert_eq!(preview.current_version, None);
    assert!(!preview.is_stale);
    assert_eq!(preview.duplicate_state, "new");
    assert!(preview.eligible);
    assert!(preview.blocker.is_none());
    assert_eq!(preview.content_problem, None);
    assert_eq!(preview.content_hash.len(), 64);
}

#[test]
fn a_preview_writes_nothing() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## From offline");

    let before = host.counts();
    for _ in 0..3 {
        host.state
            .preview_remote_submission(&meeting_id, &file.json, "submission.json")
            .expect("previews");
    }
    assert_eq!(host.counts(), before, "preview is read-only");
}

#[test]
fn a_preview_names_the_roster_name_and_flags_a_mismatch() {
    // The database name wins; the file's is display only (ADR-0022 d2, d13).
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = named(&meeting_id, &participant_id, "## Mine", "Somebody Else", 0);

    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "submission.json")
        .expect("previews");

    assert_eq!(preview.participant_name.as_deref(), Some("Budi Santoso"));
    assert_eq!(preview.submission_participant_name, "Somebody Else");
    assert!(!preview.participant_name_matches);
    // A warning, not a refusal.
    assert!(preview.eligible);
    assert!(preview.blocker.is_none());
}

#[test]
fn a_preview_marks_a_stale_form_without_blocking_it() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    host.state
        .write_participant_note(&meeting_id, &participant_id, "## Newer")
        .expect("write");

    let file = named(&meeting_id, &participant_id, "## Older", "Budi Santoso", 0);
    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "submission.json")
        .expect("previews");

    assert_eq!(preview.source_version, 0);
    assert_eq!(preview.current_version, Some(1));
    assert!(preview.is_stale);
    assert_eq!(preview.current_note.as_deref(), Some("## Newer"));
    assert!(preview.eligible, "a stale form still imports");
}

#[test]
fn a_preview_reports_every_blocking_state() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    // Wrong meeting.
    let elsewhere = MeetingId::new().to_storage();
    let file = artifact(&elsewhere, &participant_id, "## Elsewhere");
    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "f.json")
        .expect("previews");
    assert!(!preview.meeting_matches);
    assert_eq!(preview.blocker, Some(ImportBlockerDto::MeetingMismatch));
    assert!(!preview.eligible);

    // Unknown participant.
    let stranger = ParticipantId::new().to_storage();
    let file = artifact(&meeting_id, &stranger, "## Stranger");
    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "f.json")
        .expect("previews");
    assert_eq!(preview.blocker, Some(ImportBlockerDto::ParticipantNotFound));
    assert_eq!(preview.participant_id, None);

    // Content the domain would refuse.
    let file = artifact(&meeting_id, &participant_id, "<div>raw</div>");
    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "f.json")
        .expect("previews");
    assert_eq!(preview.blocker, Some(ImportBlockerDto::InvalidContent));
    assert_eq!(preview.content_problem, Some("raw_html"));

    // A locked meeting.
    let (locked, locked_participant) = host.open();
    host.state
        .domain()
        .lock_meeting(&Actor::Host, MeetingId::parse(&locked).unwrap())
        .expect("lock");
    let file = artifact(&locked, &locked_participant, "## Late");
    let preview = host
        .state
        .preview_remote_submission(&locked, &file.json, "f.json")
        .expect("previews");
    assert_eq!(preview.blocker, Some(ImportBlockerDto::MeetingLocked));
}

#[test]
fn a_preview_reports_a_duplicate_and_a_modified_artefact() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## Once");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "f.json")
        .expect("previews");
    assert_eq!(preview.duplicate_state, "duplicate");
    assert_eq!(preview.duplicate_note_version, Some(1));
    assert_eq!(preview.blocker, Some(ImportBlockerDto::Duplicate));

    let altered = re_exported(&file, "## Altered");
    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &altered.json, "f.json")
        .expect("previews");
    assert_eq!(preview.duplicate_state, "modified_artifact");
    assert_eq!(preview.blocker, Some(ImportBlockerDto::ModifiedArtifact));
}

#[test]
fn a_preview_reports_a_cross_participant_artefact() {
    let host = Host::new();
    let (meeting_id, budi, siti) = host.open_two();
    let file = artifact(&meeting_id, &budi, "## Mine");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    // The same artefact edited to name somebody else.
    let mut value: serde_json::Value = serde_json::from_str(&file.json).expect("json");
    value["participant_id"] = serde_json::json!(siti);
    let forged = value.to_string();

    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &forged, "f.json")
        .expect("previews");

    assert_eq!(preview.duplicate_state, "cross_participant");
    assert_eq!(
        preview.duplicate_participant_id,
        Some(ParticipantId::parse(&budi).unwrap())
    );
    assert_eq!(
        preview.blocker,
        Some(ImportBlockerDto::CrossParticipantArtifact)
    );
}

#[test]
fn a_malformed_artefact_is_refused_before_anything_is_read() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    for bad in [
        "not json at all",
        "[]",
        "null",
        "{\"schema_version\":2}",
        "{\"schema_version\":1,\"submission_id\":\"nope\"}",
    ] {
        let error = host
            .state
            .preview_remote_submission(&meeting_id, bad, "f.json")
            .unwrap_err();
        assert_eq!(
            error.category,
            ErrorCategory::Validation,
            "{bad}: {error:?}"
        );
    }
}

#[test]
fn an_oversized_artefact_is_refused_by_the_submission_limit() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    let huge = artifact(
        &meeting_id,
        &participant_id,
        &"a".repeat(app_remote::MAX_SUBMISSION_BYTES),
    );
    assert!(huge.json.len() > app_remote::MAX_SUBMISSION_BYTES);

    let error = host
        .state
        .preview_remote_submission(&meeting_id, &huge.json, "f.json")
        .unwrap_err();

    assert_eq!(error.category, ErrorCategory::Validation);
    assert!(error.message.contains("too large"), "{}", error.message);
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[test]
fn a_confirmed_import_writes_the_note_and_reports_the_version() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## From offline");

    let imported = host
        .state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    assert_eq!(imported.version, 1);
    assert_eq!(imported.resolution, "IMPORTED");
    assert_eq!(imported.submission_id.to_storage(), file.submission_id);
    // The roster's own name, not the one the file claimed.
    assert_eq!(imported.participant_name, "Budi Santoso");

    let note = host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .expect("a note");
    assert_eq!(note.content, "## From offline");
    assert_eq!(note.version, 1);
    assert_eq!(note.last_author_type, "REMOTE_IMPORT");
}

#[test]
fn the_host_selected_meeting_cannot_be_bypassed() {
    // The file names meeting A; the Host selected meeting B. Refused before
    // anything is resolved and long before an actor exists (ADR-0022 d1).
    let host = Host::new();
    let (first, participant_id) = host.open();
    let (second, _) = host.open();

    let file = artifact(&first, &participant_id, "## Mine");

    let before = host.counts();
    let error = host
        .state
        .import_remote_submission(&second, &file.json)
        .unwrap_err();

    assert_eq!(error.category, ErrorCategory::Validation);
    assert!(
        error.message.contains("another meeting"),
        "{}",
        error.message
    );
    assert_eq!(host.counts(), before);
}

#[test]
fn a_forged_participant_from_another_meeting_cannot_resolve() {
    // A participant id belongs to exactly one meeting, so a cross-meeting
    // artefact has nothing to resolve to.
    let host = Host::new();
    let (first, _) = host.open();
    let (_, elsewhere_participant) = host.open();

    let file = artifact(&first, &elsewhere_participant, "## Not here");

    let error = host
        .state
        .import_remote_submission(&first, &file.json)
        .unwrap_err();
    assert!(
        matches!(error.kind, HostErrorKind::ParticipantNotFound { .. }),
        "{error:?}"
    );
}

#[test]
fn a_removed_participant_is_refused_and_writes_nothing() {
    let host = Host::new();
    let meeting_id = host.draft();
    let participant_id = host.add(&meeting_id, "Budi Santoso");
    let file = artifact(&meeting_id, &participant_id, "## Gone");
    host.state
        .remove_participant(&meeting_id, &participant_id)
        .expect("remove");

    let before = host.counts();
    let error = host
        .state
        .import_remote_submission(&meeting_id, &file.json)
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::ParticipantNotFound { .. }),
        "{error:?}"
    );
    assert_eq!(host.counts(), before);
}

#[test]
fn a_duplicate_is_refused_with_the_version_the_host_already_has() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## Once");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    let before = host.counts();
    let error = host
        .state
        .import_remote_submission(&meeting_id, &file.json)
        .unwrap_err();

    match error.kind {
        HostErrorKind::DuplicateSubmission { note_version, .. } => assert_eq!(note_version, 1),
        other => panic!("expected a duplicate, got {other:?}"),
    }
    assert_eq!(error.category, ErrorCategory::Conflict);
    assert_eq!(host.counts(), before);
}

#[test]
fn a_modified_artefact_is_refused() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## Original");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    let before = host.counts();
    let error = host
        .state
        .import_remote_submission(&meeting_id, &re_exported(&file, "## Altered").json)
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::ModifiedArtifact { .. }),
        "{error:?}"
    );
    assert_eq!(host.counts(), before);
}

#[test]
fn a_cross_participant_artefact_is_refused() {
    let host = Host::new();
    let (meeting_id, budi, siti) = host.open_two();
    let file = artifact(&meeting_id, &budi, "## Mine");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    let mut value: serde_json::Value = serde_json::from_str(&file.json).expect("json");
    value["participant_id"] = serde_json::json!(siti);

    let before = host.counts();
    let error = host
        .state
        .import_remote_submission(&meeting_id, &value.to_string())
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::CrossParticipantArtifact { .. }),
        "{error:?}"
    );
    assert_eq!(host.counts(), before);
}

#[test]
fn a_meeting_locked_after_the_preview_refuses_the_confirmation() {
    // The TOCTOU case. The preview said eligible; the transaction disagrees,
    // because it re-reads the lock (ADR-0022 decision 9).
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## Too late");

    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "f.json")
        .expect("previews");
    assert!(preview.eligible);

    host.state
        .domain()
        .lock_meeting(&Actor::Host, MeetingId::parse(&meeting_id).unwrap())
        .expect("lock");

    let before = host.counts();
    let error = host
        .state
        .import_remote_submission(&meeting_id, &file.json)
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::MeetingLocked { .. }),
        "{error:?}"
    );
    assert_eq!(error.category, ErrorCategory::Lifecycle);
    assert_eq!(host.counts(), before);
}

#[test]
fn a_participant_removed_after_the_preview_refuses_the_confirmation() {
    let host = Host::new();
    let meeting_id = host.draft();
    let participant_id = host.add(&meeting_id, "Budi Santoso");
    let file = artifact(&meeting_id, &participant_id, "## Too late");

    let preview = host
        .state
        .preview_remote_submission(&meeting_id, &file.json, "f.json")
        .expect("previews");
    assert!(preview.eligible);

    host.state
        .remove_participant(&meeting_id, &participant_id)
        .expect("remove");

    assert!(host
        .state
        .import_remote_submission(&meeting_id, &file.json)
        .is_err());
}

#[test]
fn the_ledger_keeps_the_exact_bytes_that_were_imported() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    // Deliberately pretty-printed, so a re-serialisation would be visible.
    let file = artifact(&meeting_id, &participant_id, "## Verbatim");
    let pretty = serde_json::to_string_pretty(
        &serde_json::from_str::<serde_json::Value>(&file.json).expect("json"),
    )
    .expect("pretty");

    host.state
        .import_remote_submission(&meeting_id, &pretty)
        .expect("imports");

    let stored = host
        .state
        .shared_db()
        .read(|conn| {
            Ok(conn
                .query_row("SELECT raw_payload FROM remote_submissions", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("payload"))
        })
        .expect("read");

    assert_eq!(stored, pretty, "the payload must be stored verbatim");
    assert!(stored.contains('\n'), "the formatting survived");
}

#[test]
fn a_reformatted_artefact_is_still_the_same_artefact() {
    // The canonical hash is computed from the parsed fields, so whitespace and
    // key order cannot make an unchanged submission look altered.
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "## Same");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    let pretty = serde_json::to_string_pretty(
        &serde_json::from_str::<serde_json::Value>(&file.json).expect("json"),
    )
    .expect("pretty");

    let error = host
        .state
        .import_remote_submission(&meeting_id, &pretty)
        .unwrap_err();
    assert!(
        matches!(error.kind, HostErrorKind::DuplicateSubmission { .. }),
        "reformatting must not look like tampering: {error:?}"
    );
}

#[test]
fn line_endings_do_not_make_an_artefact_look_altered() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "one\ntwo\nthree");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    // The same submission with CRLF in the note, as a mail gateway might leave it.
    let crlf = re_exported(&file, "one\r\ntwo\r\nthree");
    let error = host
        .state
        .import_remote_submission(&meeting_id, &crlf.json)
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::DuplicateSubmission { .. }),
        "CRLF must normalise to the same artefact: {error:?}"
    );
}

#[test]
fn the_stored_note_is_line_ending_normalised() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    let file = artifact(&meeting_id, &participant_id, "one\r\ntwo\rthree");

    host.state
        .import_remote_submission(&meeting_id, &file.json)
        .expect("imports");

    let note = host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("read")
        .expect("a note");
    assert_eq!(note.content, "one\ntwo\nthree");
}

#[test]
fn two_distinct_artefacts_both_import_and_the_last_one_wins() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    host.state
        .import_remote_submission(
            &meeting_id,
            &artifact(&meeting_id, &participant_id, "## First").json,
        )
        .expect("first");
    let second = host
        .state
        .import_remote_submission(
            &meeting_id,
            &artifact(&meeting_id, &participant_id, "## Second").json,
        )
        .expect("second");

    assert_eq!(second.version, 2);
    assert_eq!(second.resolution, "REPLACED");
    assert_eq!(
        host.state
            .get_participant_note(&meeting_id, &participant_id)
            .expect("read")
            .expect("note")
            .content,
        "## Second"
    );
    assert_eq!(host.counts().2, 2, "two ledger rows");
}

#[test]
fn a_malformed_identifier_is_refused_by_the_import_too() {
    let host = Host::new();
    let (meeting_id, _) = host.open();

    let error = host
        .state
        .import_remote_submission(&meeting_id, "{\"schema_version\":1,\"note\":\"x\"}")
        .unwrap_err();
    assert_eq!(error.category, ErrorCategory::Validation);
}
