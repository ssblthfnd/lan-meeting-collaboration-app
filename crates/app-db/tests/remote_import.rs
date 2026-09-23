//! Remote submission import, against a real database.
//!
//! What these assert is the shape of the operation rather than its happy path:
//! that a refusal writes **nothing**, that a duplicate is a detected duplicate
//! and not a second version, and that the note, its history, the idempotency
//! ledger and the audit entry either all exist or none of them do.
//!
//! Every import goes through `Domain::import_remote_submission`, and every
//! comparison note goes through `Domain::write_note`, so what is under test is
//! the real mutation path and the state it actually produces.
//!
//! The four tables are counted before and after almost every refusal. That is
//! deliberate repetition: "nothing was written" is the property this step turns
//! on, and asserting it once would leave the other paths unproven.

use std::sync::Arc;

use app_core::actor::Actor;
use app_core::error::DomainError;
use app_core::event::{DomainEvent, EventSink};
use app_core::id::{MeetingId, ParticipantId, SubmissionId};
use app_core::meeting::MeetingConfiguration;
use app_core::participant::ParticipantDetails;
use app_core::port::SubmissionResolution;
use app_core::service::{Domain, ImportSubmission, WriteNote};
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone};
use app_db::query::{HostQueries, SubmissionLedgerState};
use app_db::Db;
use tempfile::TempDir;

/// Every event the domain published, so a test can assert what was announced.
#[derive(Default)]
struct Recorder {
    events: std::sync::Mutex<Vec<DomainEvent>>,
}

impl EventSink for Recorder {
    fn publish(&self, event: &DomainEvent) {
        self.events.lock().expect("lock").push(event.clone());
    }
}

impl Recorder {
    fn note_changes(&self) -> Vec<(ParticipantId, i64)> {
        self.events
            .lock()
            .expect("lock")
            .iter()
            .filter_map(|event| match event {
                DomainEvent::NoteChanged {
                    participant_id,
                    version,
                    ..
                } => Some((*participant_id, *version)),
                _ => None,
            })
            .collect()
    }
}

struct World {
    db: Arc<Db>,
    domain: Domain<Arc<Db>>,
    events: Arc<Recorder>,
    _dir: TempDir,
}

/// How many rows each table the import touches currently holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    notes: i64,
    versions: i64,
    submissions: i64,
    audits: i64,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open"));
        let events = Arc::new(Recorder::default());
        World {
            domain: Domain::with_events(Arc::clone(&db), Arc::clone(&events) as Arc<dyn EventSink>),
            db,
            events,
            _dir: dir,
        }
    }

    fn queries(&self) -> HostQueries<'_> {
        HostQueries::new(&self.db)
    }

    fn counts(&self) -> Counts {
        self.db
            .read(|conn| {
                let one = |sql: &str| -> i64 {
                    conn.query_row(sql, [], |row| row.get(0)).expect("count")
                };
                Ok(Counts {
                    notes: one("SELECT count(*) FROM notes"),
                    versions: one("SELECT count(*) FROM note_versions"),
                    submissions: one("SELECT count(*) FROM remote_submissions"),
                    audits: one("SELECT count(*) FROM audit_logs"),
                })
            })
            .expect("read")
    }

    /// The ledger row for one artefact, as stored.
    fn ledger(&self, submission_id: SubmissionId) -> Option<(String, String, String, i64, String)> {
        self.db
            .read(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT participant_id, content_hash, resolution, note_version, raw_payload
                           FROM remote_submissions WHERE submission_id = ?1",
                        [submission_id.to_storage()],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                            ))
                        },
                    )
                    .ok())
            })
            .expect("read")
    }

    /// Audit rows for a meeting, oldest first, as `(action, actor_type, actor_id)`.
    fn audits(&self, meeting_id: MeetingId) -> Vec<(String, String, Option<String>)> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare(
                    "SELECT action, actor_type, actor_id FROM audit_logs
                      WHERE meeting_id = ?1 ORDER BY created_at, id",
                )?;
                let rows = statement
                    .query_map([meeting_id.to_storage()], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .expect("read")
    }

    /// The newest `note_versions` row for a participant.
    fn latest_version(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> Option<(i64, String, Option<String>, String)> {
        self.db
            .read(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT v.version, v.created_by_type, v.created_by, v.content
                           FROM note_versions v
                           JOIN notes n ON n.id = v.note_id
                          WHERE n.meeting_id = ?1 AND n.participant_id = ?2
                          ORDER BY v.version DESC LIMIT 1",
                        [meeting_id.to_storage(), participant_id.to_storage()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .ok())
            })
            .expect("read")
    }

    fn meeting(&self) -> MeetingId {
        self.domain
            .create_meeting(
                &Actor::Host,
                MeetingConfiguration {
                    title: "Weekly Coordination".to_owned(),
                    topic: None,
                    date: MeetingDate::new(2026, 9, 20).unwrap(),
                    start_time: MeetingTime::new(9, 0, 0).unwrap(),
                    end_time: MeetingTime::new(10, 30, 0).unwrap(),
                    timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
                    location: None,
                    description: None,
                },
            )
            .expect("create")
            .meeting_id
    }

    fn participant(&self, meeting_id: MeetingId, name: &str) -> ParticipantId {
        self.domain
            .add_participant(
                &Actor::Host,
                meeting_id,
                ParticipantDetails {
                    name: name.to_owned(),
                    department: None,
                    position: None,
                    meeting_role: None,
                },
            )
            .expect("add")
            .participant_id
    }

    /// An open meeting with two participants, for the cross-participant cases.
    ///
    /// Both are added before the meeting opens, because the roster is settled
    /// in `DRAFT` (ADR-0013).
    fn open_two(&self) -> (MeetingId, ParticipantId, ParticipantId) {
        let meeting_id = self.meeting();
        let budi = self.participant(meeting_id, "Budi Santoso");
        let siti = self.participant(meeting_id, "Siti Rahayu");
        self.domain
            .open_meeting(&Actor::Host, meeting_id)
            .expect("open");
        (meeting_id, budi, siti)
    }

    /// An open meeting with one participant on the roster.
    fn open(&self) -> (MeetingId, ParticipantId) {
        let meeting_id = self.meeting();
        let participant_id = self.participant(meeting_id, "Budi Santoso");
        self.domain
            .open_meeting(&Actor::Host, meeting_id)
            .expect("open");
        (meeting_id, participant_id)
    }
}

/// A submission command, with everything already resolved as the Host layer
/// would have resolved it.
fn command(
    meeting_id: MeetingId,
    participant_id: ParticipantId,
    submission_id: SubmissionId,
    note: &str,
) -> ImportSubmission {
    ImportSubmission {
        meeting_id,
        participant_id,
        submission_id,
        content_hash: hash_of(meeting_id, participant_id, submission_id, note),
        content: note.to_owned(),
        raw_payload: raw_payload(meeting_id, participant_id, submission_id, note),
        source_version: 0,
    }
}

/// A stand-in for the canonical hash, with the properties the domain relies on.
///
/// The domain takes `content_hash` as an opaque 64-character hex string and
/// compares it for equality; it neither computes nor interprets one. So these
/// tests do not need the real algorithm, and deliberately do not reach for it:
/// `app-db` has no business depending on `app-remote`, and a second call site
/// for the canonical hash is the thing ADR-0022 decision 11 rules out.
///
/// What it does need is the same property the real hash has - **it changes when
/// the participant changes** - because that is what makes the cross-participant
/// lookup necessary rather than redundant.
///
/// The real algorithm is exercised where it belongs: by `app-remote`'s shared
/// fixtures, and end to end by the Tauri suite, which computes the hash it
/// hands the domain.
fn hash_of(
    meeting_id: MeetingId,
    participant_id: ParticipantId,
    submission_id: SubmissionId,
    note: &str,
) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    meeting_id.to_storage().hash(&mut hasher);
    participant_id.to_storage().hash(&mut hasher);
    submission_id.to_storage().hash(&mut hasher);
    note.hash(&mut hasher);

    // 64 lowercase hex characters, which is the column's CHECK.
    format!("{:016x}", hasher.finish()).repeat(4)
}

/// The submitted file, exactly as a participant's browser would have written it.
fn raw_payload(
    meeting_id: MeetingId,
    participant_id: ParticipantId,
    submission_id: SubmissionId,
    note: &str,
) -> String {
    serde_json::json!({
        "schema_version": 1,
        "submission_id": submission_id.to_storage(),
        "meeting_id": meeting_id.to_storage(),
        "participant_id": participant_id.to_storage(),
        "participant_name": "Budi Santoso",
        "source_version": 0,
        "generated_at": "2026-09-23T01:00:00.000Z",
        "submitted_at": "2026-09-23T04:12:07.000Z",
        "note": note,
    })
    .to_string()
}

fn actor(meeting_id: MeetingId, participant_id: ParticipantId) -> Actor {
    Actor::RemoteImport {
        meeting_id,
        participant_id,
    }
}

// ---------------------------------------------------------------------------
// The happy paths
// ---------------------------------------------------------------------------

#[test]
fn a_first_import_creates_the_note_at_version_one() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    let outcome = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## From offline"),
        )
        .expect("imports");

    assert_eq!(outcome.version, 1);
    assert!(outcome.created);
    assert_eq!(outcome.resolution, SubmissionResolution::Imported);

    let note = world
        .queries()
        .note(meeting_id, participant_id)
        .expect("read")
        .expect("a note");
    assert_eq!(note.content, "## From offline");
    assert_eq!(note.version, 1);
    assert_eq!(note.last_author_type, "REMOTE_IMPORT");
    assert_eq!(note.last_author_id, Some(participant_id));
}

#[test]
fn an_import_over_an_existing_note_replaces_it_and_appends_a_version() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();

    world
        .domain
        .write_note(
            &Actor::Host,
            WriteNote {
                meeting_id,
                participant_id,
                content: "## Written by the host".to_owned(),
            },
        )
        .expect("write");

    let outcome = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(
                meeting_id,
                participant_id,
                SubmissionId::new(),
                "## From offline",
            ),
        )
        .expect("imports");

    assert_eq!(outcome.version, 2);
    assert!(!outcome.created);
    assert_eq!(outcome.resolution, SubmissionResolution::Replaced);

    // The replaced content is still in history. Nothing is ever lost.
    let versions = world
        .queries()
        .note_versions(meeting_id, participant_id)
        .expect("read");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].created_by_type, "REMOTE_IMPORT");
    assert_eq!(versions[1].created_by_type, "HOST");
}

#[test]
fn a_draft_meeting_accepts_an_import() {
    // The lifecycle rule is the domain's: DRAFT and OPEN both permit a note
    // write, so both permit an import (ADR-0022 decision 9).
    let world = World::new();
    let meeting_id = world.meeting();
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## Early"),
        )
        .expect("a draft meeting still accepts notes");
}

#[test]
fn the_ledger_records_what_happened_and_keeps_the_payload_verbatim() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();
    let import = command(meeting_id, participant_id, submission_id, "## From offline");
    let expected_payload = import.raw_payload.clone();
    let expected_hash = import.content_hash.clone();

    world
        .domain
        .import_remote_submission(&actor(meeting_id, participant_id), import)
        .expect("imports");

    let (stored_participant, hash, resolution, note_version, payload) =
        world.ledger(submission_id).expect("a ledger row");

    assert_eq!(stored_participant, participant_id.to_storage());
    assert_eq!(hash, expected_hash);
    assert_eq!(resolution, "IMPORTED");
    assert_eq!(note_version, 1);
    // Verbatim, not a re-serialisation (ADR-0022 decision 15).
    assert_eq!(payload, expected_payload);
}

#[test]
fn a_replacement_is_recorded_as_replaced() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    world
        .domain
        .write_note(
            &Actor::Host,
            WriteNote {
                meeting_id,
                participant_id,
                content: "## First".to_owned(),
            },
        )
        .expect("write");
    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Second"),
        )
        .expect("imports");

    assert_eq!(world.ledger(submission_id).expect("row").2, "REPLACED");
}

#[test]
fn an_import_is_audited_as_the_import_it_is() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## From offline"),
        )
        .expect("imports");

    let audits = world.audits(meeting_id);
    let (action, actor_type, actor_id) = audits.last().expect("an audit row");

    assert_eq!(action, "remote_submission.imported");
    assert_eq!(actor_type, "REMOTE_IMPORT");
    assert_eq!(
        actor_id.as_deref(),
        Some(participant_id.to_storage().as_str())
    );

    // One row, not two: the note write does not also audit itself.
    assert_eq!(
        audits
            .iter()
            .filter(|(action, ..)| action.starts_with("note."))
            .count(),
        0
    );

    let metadata = world
        .db
        .read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT metadata FROM audit_logs WHERE action = 'remote_submission.imported'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("metadata"))
        })
        .expect("read");
    let metadata: serde_json::Value = serde_json::from_str(&metadata).expect("json");

    assert_eq!(metadata["submission_id"], submission_id.to_storage());
    assert_eq!(metadata["note_version"], 1);
    assert_eq!(metadata["resolution"], "IMPORTED");
    assert_eq!(metadata["source_version"], 0);
    // The note body is never in metadata, and neither is the payload.
    assert!(!metadata.to_string().contains("From offline"));
}

#[test]
fn a_successful_import_announces_the_version_and_no_content() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(
                meeting_id,
                participant_id,
                SubmissionId::new(),
                "## Secret text",
            ),
        )
        .expect("imports");

    assert_eq!(world.events.note_changes(), vec![(participant_id, 1)]);

    let announced = format!("{:?}", world.events.events.lock().expect("lock"));
    assert!(!announced.contains("Secret text"), "{announced}");
}

// ---------------------------------------------------------------------------
// Artefact identity
// ---------------------------------------------------------------------------

#[test]
fn the_same_artefact_twice_is_a_duplicate_and_writes_nothing() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Once"),
        )
        .expect("imports");

    let before = world.counts();

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Once"),
        )
        .unwrap_err();

    assert_eq!(
        error,
        DomainError::DuplicateSubmission {
            submission_id,
            note_version: 1
        }
    );
    assert_eq!(world.counts(), before, "a duplicate must write nothing");
    // And it announced nothing the second time.
    assert_eq!(world.events.note_changes().len(), 1);
}

#[test]
fn the_same_artefact_with_altered_content_is_refused() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Original"),
        )
        .expect("imports");

    let before = world.counts();

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Altered"),
        )
        .unwrap_err();

    assert_eq!(error, DomainError::ModifiedArtifact { submission_id });
    assert_eq!(world.counts(), before);

    // And the note still says what the honest artefact said.
    let note = world
        .queries()
        .note(meeting_id, participant_id)
        .expect("read")
        .expect("a note");
    assert_eq!(note.content, "## Original");
    assert_eq!(note.version, 1);
}

#[test]
fn the_same_artefact_under_another_participant_is_refused() {
    // ADR-0022 decision 6. Changing `participant_id` changes the canonical
    // hash, so the per-participant lookup cannot see this - which is the whole
    // reason the cross-participant lookup exists.
    let world = World::new();
    let (meeting_id, budi, siti) = world.open_two();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, budi),
            command(meeting_id, budi, submission_id, "## Mine"),
        )
        .expect("imports");

    let before = world.counts();

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, siti),
            command(meeting_id, siti, submission_id, "## Mine"),
        )
        .unwrap_err();

    assert_eq!(
        error,
        DomainError::CrossParticipantArtifact {
            submission_id,
            expected: budi,
            detected: siti,
        }
    );
    assert_eq!(world.counts(), before);
    assert!(world
        .queries()
        .note(meeting_id, siti)
        .expect("read")
        .is_none());
}

#[test]
fn a_different_artefact_for_the_same_participant_imports_normally() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## First"),
        )
        .expect("first");

    let second = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## Second"),
        )
        .expect("second");

    // Last write wins the content; both are in history (ADR-0022 decision 10).
    assert_eq!(second.version, 2);
    assert_eq!(
        world
            .queries()
            .note(meeting_id, participant_id)
            .expect("read")
            .expect("note")
            .content,
        "## Second"
    );
    assert_eq!(world.counts().submissions, 2);
}

#[test]
fn a_correction_is_a_new_artefact_not_a_second_export() {
    // The workflow ADR-0022 decision 6 prescribes: regenerate, do not re-export.
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let first = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, first, "## Typo"),
        )
        .expect("imports");

    // Re-exporting the old form with a fix is refused...
    assert!(world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, first, "## Fixed"),
        )
        .is_err());

    // ...and a newly generated form carries a new id and works.
    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## Fixed"),
        )
        .expect("a regenerated form imports");
}

// ---------------------------------------------------------------------------
// Refusals, and what they must not write
// ---------------------------------------------------------------------------

#[test]
fn a_locked_meeting_refuses_an_import_and_writes_nothing() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");

    let before = world.counts();

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## Late"),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::MeetingLocked { .. }),
        "{error:?}"
    );
    assert_eq!(world.counts(), before);
    assert!(world.events.note_changes().is_empty());
}

#[test]
fn a_removed_participant_refuses_an_import() {
    let world = World::new();
    let meeting_id = world.meeting();
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world
        .domain
        .remove_participant(&Actor::Host, meeting_id, participant_id)
        .expect("remove");

    let before = world.counts();

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## Gone"),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::ParticipantNotFound { .. }),
        "{error:?}"
    );
    assert_eq!(world.counts(), before);
}

#[test]
fn an_actor_confined_to_another_participant_is_refused() {
    // The transport resolves the participant and builds the actor from the row.
    // If it ever got that wrong, `authorize` refuses inside the transaction.
    let world = World::new();
    let (meeting_id, budi, siti) = world.open_two();

    let before = world.counts();

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, budi),
            command(meeting_id, siti, SubmissionId::new(), "## Not mine"),
        )
        .unwrap_err();

    assert!(matches!(error, DomainError::Forbidden { .. }), "{error:?}");
    assert_eq!(world.counts(), before);
}

#[test]
fn an_actor_from_another_meeting_has_no_standing() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let elsewhere = world.meeting();

    let error = world
        .domain
        .import_remote_submission(
            &actor(elsewhere, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## Wrong"),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::Unauthorized { .. }),
        "{error:?}"
    );
}

#[test]
fn content_the_domain_would_refuse_is_refused_here_too() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let before = world.counts();

    for bad in ["", "   ", "<div>raw html</div>", "[x](javascript:alert(1))"] {
        let error = world
            .domain
            .import_remote_submission(
                &actor(meeting_id, participant_id),
                command(meeting_id, participant_id, SubmissionId::new(), bad),
            )
            .unwrap_err();
        assert!(
            matches!(error, DomainError::Validation { .. }),
            "{bad:?}: {error:?}"
        );
    }

    assert_eq!(world.counts(), before, "invalid content must write nothing");
}

#[test]
fn an_oversized_note_is_refused_by_the_domain_limit() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(
                meeting_id,
                participant_id,
                SubmissionId::new(),
                &"a".repeat(app_core::note::MAX_NOTE_BYTES + 1),
            ),
        )
        .unwrap_err();

    assert!(matches!(error, DomainError::Validation { .. }), "{error:?}");
}

#[test]
fn a_revoked_lan_session_does_not_block_an_import() {
    // Session revocation governs LAN access. Remote participation is
    // session-less, and most remote participants never had one at all
    // (ADR-0022 decision 13).
    let world = World::new();
    let (meeting_id, participant_id) = world.open();

    world
        .domain
        .claim_identity(
            &Actor::Claimant {
                meeting_id,
                participant_id,
            },
            meeting_id,
            participant_id,
            &app_core::token::TokenHash::parse(&"a".repeat(64)).expect("a hash"),
        )
        .expect("claim");
    world
        .domain
        .revoke_session(&Actor::Host, meeting_id, participant_id)
        .expect("revoke");

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(
                meeting_id,
                participant_id,
                SubmissionId::new(),
                "## Still valid",
            ),
        )
        .expect("a revoked session is not a removed participant");
}

#[test]
fn a_stale_source_version_does_not_block_an_import() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();

    // The note moves on twice after the form was generated.
    for text in ["## One", "## Two"] {
        world
            .domain
            .write_note(
                &Actor::Host,
                WriteNote {
                    meeting_id,
                    participant_id,
                    content: text.to_owned(),
                },
            )
            .expect("write");
    }

    let mut import = command(
        meeting_id,
        participant_id,
        SubmissionId::new(),
        "## From a stale form",
    );
    import.source_version = 0;

    let outcome = world
        .domain
        .import_remote_submission(&actor(meeting_id, participant_id), import)
        .expect("a stale form still imports");

    // The version comes from the database, never from `source_version`.
    assert_eq!(outcome.version, 3);
}

// ---------------------------------------------------------------------------
// Precedence: the gate decides before artefact identity does
// ---------------------------------------------------------------------------
//
// The property these pin is an *ordering*, and orderings are the kind of thing
// that regress silently: every one of these cases would still "fail" if the
// identity lookups ran first, just with the wrong answer. A Host told
// `DuplicateSubmission` about a locked meeting would go looking for a file
// problem that is not there.

#[test]
fn a_locked_meeting_is_refused_as_locked_even_when_the_artefact_is_a_duplicate() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Once"),
        )
        .expect("imports");

    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");

    let before = world.counts();
    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Once"),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::MeetingLocked { .. }),
        "the lock must decide before artefact identity, got {error:?}"
    );
    assert_eq!(world.counts(), before);
}

#[test]
fn a_locked_meeting_is_refused_as_locked_even_when_the_artefact_was_modified() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Original"),
        )
        .expect("imports");

    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");

    let before = world.counts();
    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Altered"),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::MeetingLocked { .. }),
        "the lock must decide before artefact identity, got {error:?}"
    );
    assert_eq!(world.counts(), before);
}

#[test]
fn a_locked_meeting_is_refused_as_locked_even_for_a_cross_participant_artefact() {
    let world = World::new();
    let (meeting_id, budi, siti) = world.open_two();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, budi),
            command(meeting_id, budi, submission_id, "## Mine"),
        )
        .expect("imports");

    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");

    let before = world.counts();
    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, siti),
            command(meeting_id, siti, submission_id, "## Mine"),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::MeetingLocked { .. }),
        "the lock must decide before artefact identity, got {error:?}"
    );
    assert_eq!(world.counts(), before);
}

#[test]
fn artefact_identity_is_never_an_authorization_shortcut() {
    // An actor confined to one participant, offering an artefact that is
    // genuinely a duplicate for another. If identity were consulted first, the
    // answer would be `DuplicateSubmission` - which would mean the confinement
    // boundary had been decided by a file rather than by `authorize`.
    let world = World::new();
    let (meeting_id, budi, siti) = world.open_two();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, budi),
            command(meeting_id, budi, submission_id, "## Mine"),
        )
        .expect("imports");

    // Created before the baseline, because creating a meeting is itself an
    // audited mutation and would otherwise show up as a row this test wrote.
    let elsewhere = world.meeting();
    let before = world.counts();

    // Siti's actor, Budi's note. Refused as forbidden, not as a duplicate.
    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, siti),
            command(meeting_id, budi, submission_id, "## Mine"),
        )
        .unwrap_err();
    assert!(
        matches!(error, DomainError::Forbidden { .. }),
        "authorization must decide before artefact identity, got {error:?}"
    );

    // And an actor with no standing in this meeting at all.
    let error = world
        .domain
        .import_remote_submission(
            &actor(elsewhere, budi),
            command(meeting_id, budi, submission_id, "## Mine"),
        )
        .unwrap_err();
    assert!(
        matches!(error, DomainError::Unauthorized { .. }),
        "standing must decide before artefact identity, got {error:?}"
    );

    assert_eq!(world.counts(), before);
}

#[test]
fn a_removed_participant_is_refused_before_artefact_identity() {
    // The membership half of the gate. The artefact is a real duplicate, and
    // the answer must still be that this participant is not on the roster.
    let world = World::new();
    let meeting_id = world.meeting();
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Mine"),
        )
        .expect("imports");

    world
        .domain
        .remove_participant(&Actor::Host, meeting_id, participant_id)
        .expect("remove");

    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Mine"),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::ParticipantNotFound { .. }),
        "membership must decide before artefact identity, got {error:?}"
    );
}

#[test]
fn invalid_content_is_refused_before_artefact_identity() {
    // Content validation is part of the gate too, so a duplicate carrying
    // unusable content is reported as unusable content.
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Fine"),
        )
        .expect("imports");

    // The same artefact, now carrying raw HTML: both a modified artefact and
    // invalid content. The gate answers first.
    let error = world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(
                meeting_id,
                participant_id,
                submission_id,
                "<div>raw html</div>",
            ),
        )
        .unwrap_err();

    assert!(
        matches!(error, DomainError::Validation { .. }),
        "content validation must decide before artefact identity, got {error:?}"
    );
}

// ---------------------------------------------------------------------------
// Atomicity
// ---------------------------------------------------------------------------

#[test]
fn a_refused_import_leaves_all_four_tables_untouched() {
    // The property the whole step turns on, asserted against every refusal the
    // domain can produce after the transaction has opened.
    let world = World::new();
    let (meeting_id, participant_id, other) = world.open_two();

    // Seed one honest import so the tables are not trivially empty.
    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, SubmissionId::new(), "## Seed"),
        )
        .expect("seed");

    let before = world.counts();
    assert_eq!(before.notes, 1);
    assert_eq!(before.submissions, 1);

    let refusals: Vec<Box<dyn Fn() -> DomainError>> = vec![
        // Invalid content.
        Box::new(|| {
            world
                .domain
                .import_remote_submission(
                    &actor(meeting_id, participant_id),
                    command(meeting_id, participant_id, SubmissionId::new(), ""),
                )
                .unwrap_err()
        }),
        // Someone else's note.
        Box::new(|| {
            world
                .domain
                .import_remote_submission(
                    &actor(meeting_id, participant_id),
                    command(meeting_id, other, SubmissionId::new(), "## Not mine"),
                )
                .unwrap_err()
        }),
        // A participant that is not on any roster.
        Box::new(|| {
            let stranger = ParticipantId::new();
            world
                .domain
                .import_remote_submission(
                    &actor(meeting_id, stranger),
                    command(meeting_id, stranger, SubmissionId::new(), "## Stranger"),
                )
                .unwrap_err()
        }),
        // A meeting that does not exist.
        Box::new(|| {
            let nowhere = MeetingId::new();
            world
                .domain
                .import_remote_submission(
                    &actor(nowhere, participant_id),
                    command(nowhere, participant_id, SubmissionId::new(), "## Nowhere"),
                )
                .unwrap_err()
        }),
    ];

    for refuse in refusals {
        let error = refuse();
        assert_eq!(world.counts(), before, "{error:?} left something behind");
    }
}

#[test]
fn the_ledger_row_and_the_note_version_arrive_together() {
    let world = World::new();
    let (meeting_id, participant_id) = world.open();
    let submission_id = SubmissionId::new();

    let before = world.counts();
    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, participant_id),
            command(meeting_id, participant_id, submission_id, "## Together"),
        )
        .expect("imports");
    let after = world.counts();

    // One of each, in one transaction.
    assert_eq!(after.notes, before.notes + 1);
    assert_eq!(after.versions, before.versions + 1);
    assert_eq!(after.submissions, before.submissions + 1);
    assert_eq!(after.audits, before.audits + 1);

    // And the ledger's note_version names the version that was written.
    let (_, _, _, note_version, _) = world.ledger(submission_id).expect("a ledger row");
    let (version, created_by_type, created_by, _) = world
        .latest_version(meeting_id, participant_id)
        .expect("a version row");
    assert_eq!(note_version, version);
    assert_eq!(created_by_type, "REMOTE_IMPORT");
    assert_eq!(
        created_by.as_deref(),
        Some(participant_id.to_storage().as_str())
    );
}

// ---------------------------------------------------------------------------
// The preview's read model
// ---------------------------------------------------------------------------

#[test]
fn the_ledger_read_model_reports_what_the_transaction_will_decide() {
    let world = World::new();
    let (meeting_id, budi, siti) = world.open_two();
    let submission_id = SubmissionId::new();
    let hash = hash_of(meeting_id, budi, submission_id, "## Mine");

    // Before anything is imported.
    assert_eq!(
        world
            .queries()
            .remote_submission_state(meeting_id, budi, submission_id, &hash)
            .expect("read"),
        SubmissionLedgerState::New
    );

    world
        .domain
        .import_remote_submission(
            &actor(meeting_id, budi),
            command(meeting_id, budi, submission_id, "## Mine"),
        )
        .expect("imports");

    // Same artefact, same content.
    assert_eq!(
        world
            .queries()
            .remote_submission_state(meeting_id, budi, submission_id, &hash)
            .expect("read"),
        SubmissionLedgerState::Duplicate { note_version: 1 }
    );

    // Same artefact, altered content.
    assert_eq!(
        world
            .queries()
            .remote_submission_state(meeting_id, budi, submission_id, "0".repeat(64).as_str())
            .expect("read"),
        SubmissionLedgerState::ModifiedArtifact { note_version: 1 }
    );

    // The same artefact offered for somebody else.
    assert_eq!(
        world
            .queries()
            .remote_submission_state(
                meeting_id,
                siti,
                submission_id,
                &hash_of(meeting_id, siti, submission_id, "## Mine"),
            )
            .expect("read"),
        SubmissionLedgerState::CrossParticipant {
            participant_id: budi
        }
    );
}
