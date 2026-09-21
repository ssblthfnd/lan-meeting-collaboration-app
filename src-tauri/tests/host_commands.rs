//! The Host command layer, against a real SQLite database.
//!
//! These call [`HostState`] directly rather than through a mock Tauri app. The
//! `#[tauri::command]` functions in `commands.rs` are one-line delegations to
//! exactly these methods, so testing them here tests the whole adapter:
//! request parsing, the `Actor::Host` mint, the domain call, the query call and
//! the error mapping.
//!
//! What they are really checking is that this layer adds no rules of its own.
//! Every refusal below is a *domain* refusal arriving intact, not something the
//! adapter decided.

use app_core::participant::MAX_PARTICIPANTS;
use lan_meeting_app_lib::dto::{MeetingConfigurationInput, ParticipantDetailsInput};
use lan_meeting_app_lib::error::{ErrorCategory, HostError, HostErrorKind};
use lan_meeting_app_lib::HostState;
use serde_json::Value;
use tempfile::TempDir;

/// A Host application over a throwaway database.
struct Host {
    state: HostState,
    _dir: TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let state =
            HostState::open(dir.path().join("meetings.sqlite3")).expect("open host database");
        Host { state, _dir: dir }
    }

    /// A meeting in `DRAFT`, created the way the UI creates one.
    fn draft(&self) -> String {
        self.state
            .create_meeting(configuration())
            .expect("create meeting")
            .meeting_id
            .to_storage()
    }

    fn add(&self, meeting_id: &str, name: &str) -> String {
        self.state
            .add_participant(meeting_id, details(name))
            .expect("add participant")
            .participant_id
            .to_storage()
    }
}

fn configuration() -> MeetingConfigurationInput {
    MeetingConfigurationInput {
        title: "Weekly Coordination".to_owned(),
        topic: Some("Budget".to_owned()),
        date: "2026-09-20".to_owned(),
        start_time: "09:00:00".to_owned(),
        end_time: "10:30:00".to_owned(),
        timezone: "Asia/Makassar".to_owned(),
        location: Some("Meeting Room 2".to_owned()),
        description: Some("Coordination of weekly deliverables.".to_owned()),
    }
}

fn details(name: &str) -> ParticipantDetailsInput {
    ParticipantDetailsInput {
        name: name.to_owned(),
        department: Some("Finance".to_owned()),
        position: Some("Analyst".to_owned()),
        meeting_role: Some("Note taker".to_owned()),
    }
}

fn as_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("must serialize for the IPC boundary")
}

// ---------------------------------------------------------------------------
// 1 & 4. Creating and reading a meeting
// ---------------------------------------------------------------------------

#[test]
fn a_command_creates_a_meeting_through_the_domain() {
    let host = Host::new();

    let created = host.state.create_meeting(configuration()).unwrap();

    // The domain decides the status, and it is always DRAFT (ADR-0013).
    assert_eq!(as_json(&created)["status"], "DRAFT");

    // And the meeting is really in the database, not just reported.
    let detail = host
        .state
        .get_meeting(&created.meeting_id.to_storage())
        .unwrap();
    assert_eq!(detail.id, created.meeting_id);
    assert_eq!(detail.status.as_str(), "DRAFT");
}

#[test]
fn the_meeting_query_returns_typed_data_in_the_canonical_forms() {
    let host = Host::new();
    let meeting_id = host.draft();
    host.add(&meeting_id, "Budi Santoso");

    let detail = as_json(&host.state.get_meeting(&meeting_id).unwrap());

    assert_eq!(detail["title"], "Weekly Coordination");
    assert_eq!(detail["topic"], "Budget");
    assert_eq!(detail["location"], "Meeting Room 2");
    assert_eq!(detail["status"], "DRAFT");
    assert_eq!(detail["participant_count"], 1);
    assert_eq!(detail["locked_at"], Value::Null);

    // The zoneless schedule is carried as written, with the timezone beside it:
    // 09:00 is 09:00 in Asia/Makassar and is never converted (PRD 25.2).
    assert_eq!(detail["date"], "2026-09-20");
    assert_eq!(detail["start_time"], "09:00:00");
    assert_eq!(detail["end_time"], "10:30:00");
    assert_eq!(detail["timezone"], "Asia/Makassar");

    // Ids are canonical 36-character UUIDs and timestamps are fixed-width UTC,
    // the same representation SQLite holds (ADR-0011).
    let id = detail["id"].as_str().expect("id");
    assert_eq!(id.len(), 36);
    assert_eq!(id, id.to_lowercase());
    let created_at = detail["created_at"].as_str().expect("created_at");
    assert_eq!(created_at.len(), 24);
    assert!(created_at.ends_with('Z'), "{created_at}");

    // The list view agrees with the detail view.
    let list = host.state.list_meetings().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(as_json(&list[0])["id"], detail["id"]);
    assert_eq!(as_json(&list[0])["participant_count"], 1);
}

#[test]
fn a_meeting_that_does_not_exist_is_reported_as_not_found() {
    let host = Host::new();
    let ghost = app_core::id::MeetingId::new().to_storage();

    let error = host.state.get_meeting(&ghost).unwrap_err();
    assert_eq!(error.category, ErrorCategory::NotFound);
    assert!(matches!(error.kind, HostErrorKind::MeetingNotFound { .. }));
}

// ---------------------------------------------------------------------------
// 2. The command layer cannot bypass domain validation
// ---------------------------------------------------------------------------

#[test]
fn a_command_cannot_bypass_domain_validation() {
    let host = Host::new();

    // A blank title. Parsing accepts it; the domain refuses it.
    let mut blank = configuration();
    blank.title = "   ".to_owned();
    let error = host.state.create_meeting(blank).unwrap_err();
    assert_eq!(error.category, ErrorCategory::Validation);
    assert!(error.message.contains("meeting title"), "{}", error.message);

    // A meeting that ends before it starts.
    let mut backwards = configuration();
    backwards.start_time = "11:00:00".to_owned();
    backwards.end_time = "10:00:00".to_owned();
    let error = host.state.create_meeting(backwards).unwrap_err();
    assert_eq!(error.category, ErrorCategory::Validation);

    // A schedule inside a daylight-saving gap: 02:30 does not exist in New York
    // on this date, and the domain refuses rather than silently shifting it.
    let mut gap = configuration();
    gap.timezone = "America/New_York".to_owned();
    gap.date = "2026-03-08".to_owned();
    gap.start_time = "02:30:00".to_owned();
    gap.end_time = "04:00:00".to_owned();
    let error = host.state.create_meeting(gap).unwrap_err();
    assert_eq!(error.category, ErrorCategory::Validation);
    assert!(
        error.message.contains("daylight-saving"),
        "{}",
        error.message
    );

    // A participant with no name.
    let meeting_id = host.draft();
    let error = host
        .state
        .add_participant(&meeting_id, details("  "))
        .unwrap_err();
    assert_eq!(error.category, ErrorCategory::Validation);
    assert!(
        error.message.contains("participant name"),
        "{}",
        error.message
    );

    // Nothing was written by any of the refusals.
    assert_eq!(host.state.list_meetings().unwrap().len(), 1);
    assert_eq!(host.state.list_participants(&meeting_id).unwrap().len(), 0);
}

#[test]
fn a_malformed_identifier_is_refused_before_it_reaches_the_domain() {
    let host = Host::new();

    for bad in ["", "not-a-uuid", "9f1a8f5e-4b6d-4c3a-8f2e-1d2c3b4a5968"] {
        let error = host.state.get_meeting(bad).unwrap_err();
        // Reported as ordinary validation, so the UI needs one error path.
        assert_eq!(error.category, ErrorCategory::Validation, "{bad}");
        let json = as_json(&error);
        assert_eq!(json["field"], "meeting id", "{bad}");
        assert_eq!(json["detected"], bad);
    }
}

// ---------------------------------------------------------------------------
// 3. DomainError -> structured Host error
// ---------------------------------------------------------------------------

#[test]
fn domain_refusals_arrive_as_structured_errors_not_debug_strings() {
    let host = Host::new();
    let meeting_id = host.draft();
    let budi = host.add(&meeting_id, "Budi Santoso");
    host.state.open_meeting(&meeting_id).unwrap();

    // The roster is settled once the meeting opens (ADR-0013).
    let error = host
        .state
        .add_participant(&meeting_id, details("Late arrival"))
        .unwrap_err();

    let json = as_json(&error);
    assert_eq!(json["kind"], "meeting_not_draft");
    assert_eq!(json["category"], "lifecycle");
    assert_eq!(json["detected"], "OPEN");
    assert_eq!(json["meeting_id"], meeting_id);
    // The domain's own actionable sentence, naming expected and detected.
    let message = json["message"].as_str().expect("message");
    assert!(
        message.contains("DRAFT") && message.contains("OPEN"),
        "{message}"
    );

    // A participant that does not exist maps to its own kind.
    let ghost = app_core::id::ParticipantId::new().to_storage();
    let json = as_json(
        &host
            .state
            .remove_participant(&meeting_id, &ghost)
            .unwrap_err(),
    );
    // The lifecycle rule is checked before the lookup, so an OPEN meeting still
    // reports the lifecycle refusal - the pipeline order is preserved.
    assert_eq!(json["kind"], "meeting_not_draft");

    // In a DRAFT meeting, the same call reports the missing participant.
    let draft = host.draft();
    let json = as_json(&host.state.remove_participant(&draft, &ghost).unwrap_err());
    assert_eq!(json["kind"], "participant_not_found");
    assert_eq!(json["category"], "not_found");
    assert_eq!(json["participant_id"], ghost);

    // An invalid transition keeps its own kind too.
    let json = as_json(&host.state.open_meeting(&meeting_id).unwrap_err());
    assert_eq!(json["kind"], "invalid_transition");
    assert_eq!(json["category"], "lifecycle");
    assert_eq!(json["detected"], "OPEN");

    // And the participant added before the meeting opened is untouched.
    let roster = host.state.list_participants(&meeting_id).unwrap();
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].id.to_storage(), budi);
}

#[test]
fn a_persistence_failure_reaches_the_ui_without_its_sqlite_detail() {
    // The mapping is what matters here, so it is exercised directly: provoking a
    // real disk failure would test the operating system, not this boundary.
    let error = HostError::from(app_core::error::DomainError::Persistence {
        operation: "adding a participant",
        detail: "disk I/O error: attempt to write a readonly database".to_owned(),
    });

    assert_eq!(error.category, ErrorCategory::Unexpected);
    let json = as_json(&error).to_string();
    assert!(!json.contains("readonly database"), "leaked: {json}");
    assert!(!json.contains("disk I/O"), "leaked: {json}");
    // Still says which operation failed.
    assert!(json.contains("adding a participant"), "{json}");
}

// ---------------------------------------------------------------------------
// 5 & 8. Participant management through the command layer
// ---------------------------------------------------------------------------

#[test]
fn the_participant_query_returns_typed_data_ordered_by_name() {
    let host = Host::new();
    let meeting_id = host.draft();

    // Inserted out of order on purpose.
    for name in ["Siti Rahayu", "Ahmad Fauzi", "Budi Santoso"] {
        host.add(&meeting_id, name);
    }

    let roster = host.state.list_participants(&meeting_id).unwrap();
    let names: Vec<&str> = roster.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ahmad Fauzi", "Budi Santoso", "Siti Rahayu"]);

    let first = as_json(&roster[0]);
    assert_eq!(first["name"], "Ahmad Fauzi");
    assert_eq!(first["department"], "Finance");
    assert_eq!(first["position"], "Analyst");
    assert_eq!(first["meeting_role"], "Note taker");
    assert_eq!(
        first["id"].as_str().expect("id").len(),
        36,
        "ids cross the boundary in canonical form"
    );
}

#[test]
fn participant_management_through_commands_reaches_the_domain_rules() {
    let host = Host::new();
    let meeting_id = host.draft();

    let added = host
        .state
        .add_participant(&meeting_id, details("Budi"))
        .unwrap();
    assert_eq!(added.roster_size, 1);

    let participant_id = added.participant_id.to_storage();
    host.state
        .update_participant(
            &meeting_id,
            &participant_id,
            ParticipantDetailsInput {
                name: "Budi Santoso".to_owned(),
                department: Some("Operations".to_owned()),
                position: None,
                // Blank optional text becomes absent, normalised by the domain.
                meeting_role: Some("   ".to_owned()),
            },
        )
        .unwrap();

    let roster = host.state.list_participants(&meeting_id).unwrap();
    assert_eq!(roster[0].name, "Budi Santoso");
    assert_eq!(roster[0].department.as_deref(), Some("Operations"));
    assert_eq!(roster[0].position, None);
    assert_eq!(roster[0].meeting_role, None);

    let removed = host
        .state
        .remove_participant(&meeting_id, &participant_id)
        .unwrap();
    assert_eq!(removed.roster_size, 0);
    assert!(host
        .state
        .list_participants(&meeting_id)
        .unwrap()
        .is_empty());

    // A participant id from another meeting is not reachable through this one.
    let other = host.draft();
    let outsider = host.add(&other, "Outsider");
    let error = host
        .state
        .update_participant(&meeting_id, &outsider, details("Renamed"))
        .unwrap_err();
    assert!(matches!(
        error.kind,
        HostErrorKind::ParticipantNotFound { .. }
    ));
}

// ---------------------------------------------------------------------------
// 9. The 99-participant limit through the Tauri path
// ---------------------------------------------------------------------------

#[test]
fn the_hundredth_participant_still_fails_through_the_command_layer() {
    let host = Host::new();
    let meeting_id = host.draft();

    for n in 1..=MAX_PARTICIPANTS {
        let added = host
            .state
            .add_participant(&meeting_id, details(&format!("Person {n}")))
            .unwrap_or_else(|e| panic!("participant {n} should be allowed: {e:?}"));
        assert_eq!(added.roster_size, n as i64);
    }

    let error = host
        .state
        .add_participant(&meeting_id, details("Person 100"))
        .unwrap_err();

    // The limit is the domain's; the adapter just carries the refusal.
    let json = as_json(&error);
    assert_eq!(json["kind"], "validation");
    assert_eq!(json["category"], "validation");
    assert_eq!(json["field"], "participant count");
    let message = json["message"].as_str().expect("message");
    assert!(message.contains("at most 99"), "{message}");
    assert!(message.contains("already has 99"), "{message}");

    assert_eq!(host.state.list_participants(&meeting_id).unwrap().len(), 99);
    assert_eq!(
        host.state
            .get_meeting(&meeting_id)
            .unwrap()
            .participant_count,
        99
    );
}

// ---------------------------------------------------------------------------
// 10. Lifecycle refusals propagate
// ---------------------------------------------------------------------------

#[test]
fn configuration_and_roster_refusals_propagate_once_the_meeting_is_open() {
    let host = Host::new();
    let meeting_id = host.draft();
    let budi = host.add(&meeting_id, "Budi Santoso");

    // While DRAFT, reconfiguring works.
    let mut revised = configuration();
    revised.title = "Weekly Coordination (revised)".to_owned();
    host.state.update_meeting(&meeting_id, revised).unwrap();
    assert_eq!(
        host.state.get_meeting(&meeting_id).unwrap().title,
        "Weekly Coordination (revised)"
    );

    host.state.open_meeting(&meeting_id).unwrap();
    let before = host.state.get_meeting(&meeting_id).unwrap();

    let mut too_late = configuration();
    too_late.title = "Too late".to_owned();

    let refusals = [
        host.state
            .update_meeting(&meeting_id, too_late)
            .unwrap_err(),
        host.state
            .add_participant(&meeting_id, details("Late arrival"))
            .unwrap_err(),
        host.state
            .update_participant(&meeting_id, &budi, details("Renamed"))
            .unwrap_err(),
        host.state
            .remove_participant(&meeting_id, &budi)
            .unwrap_err(),
    ];

    for error in &refusals {
        assert_eq!(error.category, ErrorCategory::Lifecycle);
        assert!(
            matches!(error.kind, HostErrorKind::MeetingNotDraft { .. }),
            "{error:?}"
        );
    }

    // Nothing changed.
    let after = host.state.get_meeting(&meeting_id).unwrap();
    assert_eq!(after.title, before.title);
    assert_eq!(after.updated_at, before.updated_at);
    assert_eq!(host.state.list_participants(&meeting_id).unwrap().len(), 1);
}

#[test]
fn a_locked_meeting_refuses_through_the_command_layer_too() {
    let host = Host::new();
    let meeting_id = host.draft();
    host.state.open_meeting(&meeting_id).unwrap();

    // Locking is not exposed as a command on purpose (it is not part of this
    // step), so the domain is used directly to reach the LOCKED state.
    let parsed = app_core::id::MeetingId::parse(&meeting_id).unwrap();
    host.state
        .domain()
        .lock_meeting(&app_core::actor::Actor::Host, parsed)
        .unwrap();

    let error = host
        .state
        .add_participant(&meeting_id, details("Nobody"))
        .unwrap_err();

    // The lock is the stronger refusal and is checked first.
    assert_eq!(error.category, ErrorCategory::Lifecycle);
    let json = as_json(&error);
    assert_eq!(json["kind"], "meeting_locked");
    assert!(json["message"]
        .as_str()
        .expect("message")
        .contains("Weekly Coordination"));

    // The detail view reports it, so the UI can say so rather than guessing.
    let detail = host.state.get_meeting(&meeting_id).unwrap();
    assert_eq!(detail.status.as_str(), "LOCKED");
    assert!(detail.locked_at.is_some());
}

// ---------------------------------------------------------------------------
// 6. Audit
// ---------------------------------------------------------------------------

#[test]
fn the_audit_query_is_deterministic_and_complete() {
    let host = Host::new();
    let meeting_id = host.draft();
    let budi = host.add(&meeting_id, "Budi Santoso");
    host.state
        .update_meeting(&meeting_id, configuration())
        .unwrap();
    host.state
        .update_participant(&meeting_id, &budi, details("Budi S."))
        .unwrap();
    host.state.remove_participant(&meeting_id, &budi).unwrap();
    host.state.open_meeting(&meeting_id).unwrap();

    let entries = host.state.list_audit_entries(&meeting_id).unwrap();
    let actions: Vec<&str> = entries.iter().map(|e| e.action.as_str()).collect();

    // Chronological, and every action this step can produce is distinguishable.
    assert_eq!(
        actions,
        [
            "meeting.created",
            "participant.added",
            "meeting.updated",
            "participant.updated",
            "participant.removed",
            "meeting.opened",
        ]
    );

    // Repeating the query gives the same order: it is total, not incidental.
    for _ in 0..3 {
        let again = host.state.list_audit_entries(&meeting_id).unwrap();
        assert_eq!(
            again.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
            entries.iter().map(|e| e.id.clone()).collect::<Vec<_>>()
        );
    }

    // Timestamps are non-decreasing, which is what makes the order meaningful.
    let stamps: Vec<String> = entries
        .iter()
        .map(|e| as_json(&e.created_at).as_str().expect("stamp").to_owned())
        .collect();
    let mut sorted = stamps.clone();
    sorted.sort();
    assert_eq!(stamps, sorted);

    let first = as_json(&entries[0]);
    assert_eq!(first["actor_type"], "HOST");
    // The Host is not a participant, so no author id (ADR-0008).
    assert_eq!(first["actor_id"], Value::Null);
    assert_eq!(first["target_type"], "meeting");
    assert_eq!(first["target_id"], meeting_id);
    // Metadata arrives as a JSON object, not as a string to re-parse.
    assert!(first["metadata"].is_object(), "{first}");
    assert_eq!(first["metadata"]["timezone"], "Asia/Makassar");

    // The removal keeps the participant's details, because the row is gone.
    let removal = entries
        .iter()
        .find(|e| e.action == "participant.removed")
        .expect("removal audited");
    let json = as_json(removal);
    assert_eq!(json["target_type"], "participant");
    assert_eq!(json["target_id"], budi);
    assert_eq!(json["metadata"]["name"], "Budi S.");

    // Audit entries are scoped to their meeting.
    let other = host.draft();
    assert_eq!(host.state.list_audit_entries(&other).unwrap().len(), 1);
}

#[test]
fn a_refused_mutation_adds_no_audit_entry() {
    let host = Host::new();
    let meeting_id = host.draft();
    let before = host.state.list_audit_entries(&meeting_id).unwrap().len();

    let mut blank = configuration();
    blank.title = " ".to_owned();
    assert!(host.state.update_meeting(&meeting_id, blank).is_err());
    assert!(host
        .state
        .add_participant(&meeting_id, details(""))
        .is_err());

    assert_eq!(
        host.state.list_audit_entries(&meeting_id).unwrap().len(),
        before,
        "a refusal must leave no trace, including in the audit log"
    );
}

// ---------------------------------------------------------------------------
// Reads do not take the write path
// ---------------------------------------------------------------------------

#[test]
fn reads_do_not_block_on_the_write_lock() {
    // A read runs on the read-only pool (ADR-0014), so a query never waits for
    // the writer and could not perform a write even if asked to. Proven here by
    // reading from many threads while writes proceed on the same database.
    let host = Host::new();
    let meeting_id = host.draft();
    let state = &host.state;

    std::thread::scope(|scope| {
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let id = meeting_id.clone();
                scope.spawn(move || {
                    for _ in 0..25 {
                        state.list_meetings().expect("list");
                        state.get_meeting(&id).expect("detail");
                        state.list_participants(&id).expect("roster");
                        state.list_audit_entries(&id).expect("audit");
                    }
                })
            })
            .collect();

        for n in 0..25 {
            state
                .add_participant(&meeting_id, details(&format!("Person {n}")))
                .expect("add");
        }

        for reader in readers {
            reader.join().expect("reader thread");
        }
    });

    assert_eq!(host.state.list_participants(&meeting_id).unwrap().len(), 25);
}
