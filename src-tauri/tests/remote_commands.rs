//! Remote form generation, against a real SQLite database and a real filesystem.
//!
//! These call [`HostState`] directly, for the reason `host_commands.rs` gives:
//! the `#[tauri::command]` function is a one-line delegation to exactly this
//! method, so what is under test is the whole adapter - parsing, the reads, the
//! lifecycle refusal, the generation call and the file write.
//!
//! The output directory is a parameter rather than something the state holds,
//! which is what makes this testable at all. In the running application the
//! command layer resolves it from Tauri's application-data directory, and the
//! window never sees it: a renderer that could name a path would be a renderer
//! that could write anywhere (ADR-0021 decision 10).
//!
//! Generation writes nothing to the database, and several tests below assert
//! that directly. The `remote_submissions` ledger row belongs to an *import*,
//! and import is step 10.

use std::path::Path;

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

    /// Where generated forms go in these tests.
    fn output(&self) -> std::path::PathBuf {
        self.dir.path().join("remote-forms")
    }

    fn draft(&self) -> String {
        self.state
            .create_meeting(configuration())
            .expect("create")
            .meeting_id
            .to_storage()
    }

    /// An opened meeting with one participant on the roster.
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

/// Whether a real template is embedded, so these can skip on a clean checkout.
fn template_available() -> bool {
    app_remote::is_bundled()
}

macro_rules! require_template {
    () => {
        if !template_available() {
            eprintln!("[host] skipped: no remote form template. Run `npm run build:remote`.");
            return;
        }
    };
}

/// The island's contents, so a test can read back what was baked in.
fn island_payload(html: &str) -> String {
    const OPEN: &str = r#"<script type="application/json" id="submission-context">"#;
    let start = html.find(OPEN).expect("an island") + OPEN.len();
    let end = start + html[start..].find("</script>").expect("a closing tag");
    html[start..end].to_owned()
}

// ---------------------------------------------------------------------------
// The happy path
// ---------------------------------------------------------------------------

#[test]
fn generating_writes_a_file_at_the_path_it_reports() {
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    let generated = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("generates");

    // The returned path is the file, not a folder or a hopeful guess.
    let path = Path::new(&generated.path);
    assert!(path.is_file(), "{}", generated.path);
    assert!(
        generated.path.ends_with(&generated.file_name),
        "{generated:?}"
    );

    let written = std::fs::read_to_string(path).expect("readable");
    assert_eq!(written.len() as u64, generated.bytes);
    assert!(written.starts_with("<!doctype html"));

    // And it is the artefact `app-remote` vouches for.
    app_remote::verify_artifact(&written).expect("self-contained");
}

#[test]
fn the_form_carries_the_meeting_the_participant_and_the_submission_id() {
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    let generated = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("generates");

    let written = std::fs::read_to_string(&generated.path).expect("readable");
    let context: serde_json::Value =
        serde_json::from_str(&island_payload(&written)).expect("a readable context");

    assert_eq!(context["meeting_id"], meeting_id);
    assert_eq!(context["participant_id"], participant_id);
    assert_eq!(
        context["submission_id"],
        generated.submission_id.to_storage()
    );
    assert_eq!(context["meeting_title"], "Weekly Coordination");
    assert_eq!(context["participant_name"], "Budi Santoso");
    // The date is meaningless without the zone, so both are present.
    assert_eq!(context["meeting_date"], "2026-09-20");
    assert_eq!(context["meeting_timezone"], "Asia/Makassar");
    assert_eq!(context["schema_version"], 1);
}

#[test]
fn an_existing_note_is_prefilled_with_its_version() {
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    host.state
        .write_participant_note(
            &meeting_id,
            &participant_id,
            "## Already written\n\n- A point",
        )
        .expect("write");
    host.state
        .write_participant_note(&meeting_id, &participant_id, "## Rewritten")
        .expect("rewrite");

    let generated = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("generates");

    assert_eq!(generated.source_version, 2);

    let written = std::fs::read_to_string(&generated.path).expect("readable");
    let context: serde_json::Value = serde_json::from_str(&island_payload(&written)).unwrap();
    assert_eq!(context["existing_content"], "## Rewritten");
    assert_eq!(context["source_version"], 2);
}

#[test]
fn a_participant_without_a_note_gets_an_empty_form() {
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    let generated = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("generates");

    assert_eq!(generated.source_version, 0);

    let written = std::fs::read_to_string(&generated.path).expect("readable");
    let context: serde_json::Value = serde_json::from_str(&island_payload(&written)).unwrap();
    assert_eq!(context["existing_content"], "");
    assert_eq!(context["source_version"], 0);
}

#[test]
fn generating_twice_produces_two_artefacts_and_keeps_both() {
    // ADR-0021 decision 13: a newer form does not invalidate an older one.
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    let first = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("first");
    let second = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("second");

    assert_ne!(first.submission_id, second.submission_id);
    assert_ne!(
        first.path, second.path,
        "the second must not overwrite the first"
    );
    assert!(Path::new(&first.path).is_file());
    assert!(Path::new(&second.path).is_file());
}

#[test]
fn the_meeting_is_generated_for_the_participant_that_was_asked_for() {
    require_template!();
    let host = Host::new();
    let meeting_id = host.draft();

    let budi = host
        .state
        .add_participant(&meeting_id, details("Budi Santoso"))
        .expect("add")
        .participant_id
        .to_storage();
    let siti = host
        .state
        .add_participant(&meeting_id, details("Siti Rahayu"))
        .expect("add")
        .participant_id
        .to_storage();
    host.state.open_meeting(&meeting_id).expect("open");

    let generated = host
        .state
        .generate_remote_form(&meeting_id, &siti, &host.output())
        .expect("generates");

    let written = std::fs::read_to_string(&generated.path).expect("readable");
    let context: serde_json::Value = serde_json::from_str(&island_payload(&written)).unwrap();

    assert_eq!(context["participant_id"], siti);
    assert_eq!(context["participant_name"], "Siti Rahayu");
    assert!(
        !written.contains(&budi),
        "the other participant's id leaked"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_meeting_is_refused() {
    let host = Host::new();
    let (_, participant_id) = host.open();
    let elsewhere = app_core::id::MeetingId::new().to_storage();

    let error = host
        .state
        .generate_remote_form(&elsewhere, &participant_id, &host.output())
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::MeetingNotFound { .. }),
        "{error:?}"
    );
    assert_eq!(error.category, ErrorCategory::NotFound);
    assert!(
        !host.output().exists(),
        "nothing may be written for a refusal"
    );
}

#[test]
fn an_unknown_participant_is_refused() {
    let host = Host::new();
    let (meeting_id, _) = host.open();
    let stranger = app_core::id::ParticipantId::new().to_storage();

    let error = host
        .state
        .generate_remote_form(&meeting_id, &stranger, &host.output())
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::ParticipantNotFound { .. }),
        "{error:?}"
    );
    assert!(
        !host.output().exists(),
        "nothing may be written for a refusal"
    );
}

#[test]
fn a_participant_of_another_meeting_is_refused() {
    // The roster read is meeting-scoped, so somebody else's participant is
    // absent rather than borrowed.
    let host = Host::new();
    let (first, _) = host.open();
    let (_, other_participant) = host.open();

    let error = host
        .state
        .generate_remote_form(&first, &other_participant, &host.output())
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::ParticipantNotFound { .. }),
        "{error:?}"
    );
}

#[test]
fn a_malformed_identifier_is_refused_before_anything_is_read() {
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    for (meeting, participant) in [
        ("not-a-uuid", participant_id.as_str()),
        (meeting_id.as_str(), "not-a-uuid"),
    ] {
        let error = host
            .state
            .generate_remote_form(meeting, participant, &host.output())
            .unwrap_err();
        assert_eq!(error.category, ErrorCategory::Validation, "{error:?}");
    }
}

#[test]
fn a_locked_meeting_refuses_generation() {
    // ADR-0021 decision 2: the lifecycle rule is the domain's, and a form that
    // could never be imported is not worth handing to anybody.
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();
    host.state
        .domain()
        .lock_meeting(
            &app_core::actor::Actor::Host,
            app_core::id::MeetingId::parse(&meeting_id).unwrap(),
        )
        .expect("lock");

    let error = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::MeetingLocked { .. }),
        "{error:?}"
    );
    assert_eq!(error.category, ErrorCategory::Lifecycle);
    assert!(
        !host.output().exists(),
        "nothing may be written for a refusal"
    );
}

#[test]
fn a_draft_meeting_may_still_be_generated_for() {
    // DRAFT and OPEN both accept note writes, so both accept a form. Only
    // LOCKED refuses (ADR-0021 decision 2).
    require_template!();
    let host = Host::new();
    let meeting_id = host.draft();
    let participant_id = host
        .state
        .add_participant(&meeting_id, details("Budi Santoso"))
        .expect("add")
        .participant_id
        .to_storage();

    host.state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("a draft meeting may be prepared for");
}

#[test]
fn an_unwritable_directory_is_reported_as_the_hosts_own_problem() {
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    // A file where the directory should be: `create_dir_all` cannot succeed.
    let blocked = host.dir.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("write the blocker");

    let error = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &blocked)
        .unwrap_err();

    assert!(
        matches!(error.kind, HostErrorKind::Persistence),
        "{error:?}"
    );
    assert_eq!(error.category, ErrorCategory::Unexpected);
    // The Host is told where, because it is their own machine.
    assert!(error.message.contains("blocked"), "{}", error.message);
}

// ---------------------------------------------------------------------------
// What generation must not do
// ---------------------------------------------------------------------------

#[test]
fn generating_writes_nothing_to_the_database() {
    // No note, no version, no audit entry. The ledger row belongs to an import,
    // and import is step 10.
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    let audit_before = host
        .state
        .list_audit_entries(&meeting_id)
        .expect("audit")
        .len();

    host.state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("generates");

    assert_eq!(
        host.state
            .list_audit_entries(&meeting_id)
            .expect("audit")
            .len(),
        audit_before,
        "generation is a read and a file, not a mutation"
    );
    assert!(host
        .state
        .get_participant_note(&meeting_id, &participant_id)
        .expect("note")
        .is_none());
    assert!(host
        .state
        .list_note_versions(&meeting_id, &participant_id)
        .expect("versions")
        .is_empty());
}

#[test]
fn a_generated_form_carries_no_credential() {
    require_template!();
    let host = Host::new();
    let (meeting_id, participant_id) = host.open();

    // Issue a join token first, so there is a real secret in this process to
    // leak. The generated file must contain none of it.
    let issued = host
        .state
        .issue_join_token(&meeting_id, "192.168.1.42", 8765)
        .expect("issue");

    let generated = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("generates");
    let written = std::fs::read_to_string(&generated.path).expect("readable");

    assert!(!written.contains(&issued.join_url), "the join URL leaked");
    for forbidden in [
        "Bearer ",
        "session_token",
        "join_token",
        "token_hash",
        "192.168.1.42",
        "/join/",
    ] {
        assert!(
            !written.contains(forbidden),
            "the form contains `{forbidden}`"
        );
    }
}

#[test]
fn hostile_participant_and_meeting_names_cannot_break_the_artefact() {
    require_template!();
    let host = Host::new();

    let meeting_id = host
        .state
        .create_meeting(MeetingConfigurationInput {
            title: "</script><img src=x onerror=alert(1)>".to_owned(),
            ..configuration()
        })
        .expect("create")
        .meeting_id
        .to_storage();

    let participant_id = host
        .state
        .add_participant(&meeting_id, details("</SCRIPT ><iframe>"))
        .expect("add")
        .participant_id
        .to_storage();

    host.state
        .write_participant_note(
            &meeting_id,
            &participant_id,
            "A note mentioning `</script>` in a code span\n\nand a [link](https://example.com)",
        )
        .expect("write");

    let generated = host
        .state
        .generate_remote_form(&meeting_id, &participant_id, &host.output())
        .expect("generates");
    let written = std::fs::read_to_string(&generated.path).expect("readable");

    app_remote::verify_artifact(&written).expect("still self-contained");
    assert!(
        !island_payload(&written).contains('<'),
        "the island was breached"
    );

    // The file name is a convenience and is slugified before it is a path.
    assert!(
        generated
            .file_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'),
        "{}",
        generated.file_name
    );
    assert!(
        !generated.file_name.contains(".."),
        "{}",
        generated.file_name
    );
}
