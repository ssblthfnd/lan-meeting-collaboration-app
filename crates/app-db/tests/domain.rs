//! Domain rule engine tests, against a real SQLite database.
//!
//! These live in `app-db` rather than `app-core` because the rules are only
//! meaningful against real transactions: `BEGIN IMMEDIATE`, real constraint
//! violations and real concurrent writers. `app-core` keeps the tests that need
//! no database (the authorization matrix, the lifecycle table, validation).
//!
//! Nothing here mocks locking. Where a test is about concurrency it runs actual
//! threads against one database and asserts on the committed result, so it
//! cannot pass by accident of timing - and there are no sleeps to tune.

use app_core::actor::Actor;
use app_core::error::DomainError;
use app_core::id::{MeetingId, ParticipantId, SessionId};
use app_core::meeting::MeetingStatus;
use app_core::service::{Domain, WriteNote};
use app_core::time::{MeetingDate, MeetingTime, UtcTimestamp};
use app_db::{Db, Sql};
use rusqlite::params;
use std::sync::Arc;
use tempfile::TempDir;

/// A migrated database plus the domain boundary wrapped around it.
struct World {
    // A separate handle purely so tests can *read* the result. The boundary
    // itself is `domain`; nothing here writes except through it, apart from the
    // seed helpers below, which stand in for the meeting-creation flow that
    // belongs to a later step.
    db: Arc<Db>,
    domain: Domain<Arc<Db>>,
    _dir: TempDir,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open database"));
        World {
            domain: Domain::new(Arc::clone(&db)),
            db,
            _dir: dir,
        }
    }

    fn db(&self) -> &Db {
        &self.db
    }

    fn seed_meeting(&self, status: MeetingStatus) -> MeetingId {
        let id = MeetingId::new();
        let now = UtcTimestamp::now();
        let locked_at = (status == MeetingStatus::Locked).then_some(Sql(now));

        self.db()
            .write(|tx| {
                tx.execute(
                    "INSERT INTO meetings
                       (id, title, date, start_time, end_time, timezone, status,
                        created_at, updated_at, locked_at)
                     VALUES (?1, 'Weekly Coordination', ?2, ?3, ?4, 'Asia/Makassar', ?5,
                             ?6, ?6, ?7)",
                    params![
                        Sql(id),
                        Sql(MeetingDate::new(2026, 9, 20).unwrap()),
                        Sql(MeetingTime::new(9, 0, 0).unwrap()),
                        Sql(MeetingTime::new(10, 30, 0).unwrap()),
                        status.as_str(),
                        Sql(now),
                        locked_at,
                    ],
                )?;
                Ok(())
            })
            .expect("seed meeting");
        id
    }

    fn seed_participant(&self, meeting_id: MeetingId, name: &str) -> ParticipantId {
        let id = ParticipantId::new();
        self.db()
            .write(|tx| {
                tx.execute(
                    "INSERT INTO participants (id, meeting_id, name, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![Sql(id), Sql(meeting_id), name, Sql(UtcTimestamp::now())],
                )?;
                Ok(())
            })
            .expect("seed participant");
        id
    }

    fn count(&self, sql: &str) -> i64 {
        self.db()
            .read(|conn| Ok(conn.query_row(sql, [], |row| row.get(0))?))
            .expect("count")
    }

    fn notes(&self) -> i64 {
        self.count("SELECT count(*) FROM notes")
    }

    fn versions(&self) -> i64 {
        self.count("SELECT count(*) FROM note_versions")
    }

    fn audits(&self) -> i64 {
        self.count("SELECT count(*) FROM audit_logs")
    }

    fn meeting_status(&self, meeting_id: MeetingId) -> String {
        self.db()
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT status FROM meetings WHERE id = ?1",
                    params![Sql(meeting_id)],
                    |row| row.get(0),
                )?)
            })
            .expect("status")
    }
}

fn participant_actor(meeting_id: MeetingId, participant_id: ParticipantId) -> Actor {
    Actor::Participant {
        meeting_id,
        participant_id,
        session_id: SessionId::new(),
    }
}

fn note(meeting_id: MeetingId, participant_id: ParticipantId, content: &str) -> WriteNote {
    WriteNote {
        meeting_id,
        participant_id,
        content: content.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// 5. A host can mutate an authorized resource
// ---------------------------------------------------------------------------

#[test]
fn a_host_can_open_lock_and_write_notes() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Draft);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    let opened = w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();
    assert_eq!(opened.from, MeetingStatus::Draft);
    assert_eq!(opened.to, MeetingStatus::Open);
    assert_eq!(w.meeting_status(meeting_id), "OPEN");

    // The Host may write any participant's note (PRD section 5.1).
    let written = w
        .domain
        .write_note(&Actor::Host, note(meeting_id, budi, "## Agenda"))
        .unwrap();
    assert!(written.created);
    assert_eq!(written.version, 1);

    let locked = w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();
    assert_eq!(locked.to, MeetingStatus::Locked);
    assert_eq!(w.meeting_status(meeting_id), "LOCKED");
}

#[test]
fn a_host_note_edit_is_recorded_as_a_host_edit_with_no_author_id() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    w.domain
        .write_note(&Actor::Host, note(meeting_id, budi, "Host wrote this"))
        .unwrap();

    let (kind, author): (String, Option<String>) = w
        .db()
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT created_by_type, created_by FROM note_versions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();

    // The schema requires created_by IS NULL exactly when the type is HOST,
    // and the Host is not a participant even when editing one's note.
    assert_eq!(kind, "HOST");
    assert_eq!(author, None);
}

#[test]
fn a_participant_edit_records_the_participant_as_author() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    w.domain
        .write_note(
            &participant_actor(meeting_id, budi),
            note(meeting_id, budi, "My own note"),
        )
        .unwrap();

    let (kind, author): (String, Option<Sql<ParticipantId>>) = w
        .db()
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT created_by_type, created_by FROM note_versions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();

    assert_eq!(kind, "PARTICIPANT");
    assert_eq!(author.map(Sql::into_inner), Some(budi));
}

#[test]
fn a_remote_import_is_recorded_as_a_remote_import() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let siti = w.seed_participant(meeting_id, "Siti");

    let actor = Actor::RemoteImport {
        meeting_id,
        participant_id: siti,
    };
    w.domain
        .write_note(&actor, note(meeting_id, siti, "Sent from the offline form"))
        .unwrap();

    let kind: String = w
        .db()
        .read(|conn| {
            Ok(
                conn.query_row("SELECT created_by_type FROM note_versions", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(kind, "REMOTE_IMPORT");
}

// ---------------------------------------------------------------------------
// 3 & 4. Participants cannot exceed their scope
// ---------------------------------------------------------------------------

#[test]
fn a_participant_cannot_write_another_participants_note() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");
    let siti = w.seed_participant(meeting_id, "Siti");

    let err = w
        .domain
        .write_note(
            &participant_actor(meeting_id, budi),
            note(meeting_id, siti, "Not mine to write"),
        )
        .unwrap_err();

    assert!(matches!(err, DomainError::Forbidden { .. }), "{err:?}");
    assert_eq!(w.notes(), 0);
    assert_eq!(w.versions(), 0);
    assert_eq!(w.audits(), 0);
}

#[test]
fn a_participant_cannot_change_meeting_configuration() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Draft);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");
    let actor = participant_actor(meeting_id, budi);

    let open_err = w.domain.open_meeting(&actor, meeting_id).unwrap_err();
    assert!(
        matches!(open_err, DomainError::Forbidden { .. }),
        "{open_err:?}"
    );

    // And still cannot lock it once it is open.
    w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();
    let lock_err = w.domain.lock_meeting(&actor, meeting_id).unwrap_err();
    assert!(
        matches!(lock_err, DomainError::Forbidden { .. }),
        "{lock_err:?}"
    );

    assert_eq!(w.meeting_status(meeting_id), "OPEN");
}

#[test]
fn a_participant_from_another_meeting_has_no_standing_here() {
    let w = World::new();
    let theirs = w.seed_meeting(MeetingStatus::Open);
    let ours = w.seed_meeting(MeetingStatus::Open);
    let outsider = w.seed_participant(theirs, "Outsider");
    let insider = w.seed_participant(ours, "Insider");

    let err = w
        .domain
        .write_note(
            &participant_actor(theirs, outsider),
            note(ours, insider, "trespass"),
        )
        .unwrap_err();

    assert_eq!(err, DomainError::Unauthorized { meeting_id: ours });
    assert_eq!(w.notes(), 0);
}

#[test]
fn a_remote_import_cannot_reach_beyond_its_participant() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");
    let siti = w.seed_participant(meeting_id, "Siti");

    let actor = Actor::RemoteImport {
        meeting_id,
        participant_id: budi,
    };

    let err = w
        .domain
        .write_note(&actor, note(meeting_id, siti, "wrong subject"))
        .unwrap_err();
    assert!(matches!(err, DomainError::Forbidden { .. }), "{err:?}");

    let lock_err = w.domain.lock_meeting(&actor, meeting_id).unwrap_err();
    assert!(
        matches!(lock_err, DomainError::Forbidden { .. }),
        "{lock_err:?}"
    );
}

// ---------------------------------------------------------------------------
// 8. Invalid state transitions
// ---------------------------------------------------------------------------

#[test]
fn invalid_meeting_state_transitions_are_rejected() {
    let w = World::new();
    let draft = w.seed_meeting(MeetingStatus::Draft);

    // DRAFT cannot jump straight to LOCKED.
    let err = w.domain.lock_meeting(&Actor::Host, draft).unwrap_err();
    assert_eq!(
        err,
        DomainError::InvalidTransition {
            expected: "OPEN",
            detected: MeetingStatus::Draft,
        }
    );
    assert_eq!(w.meeting_status(draft), "DRAFT");
    assert_eq!(w.audits(), 0, "a rejected transition must not be audited");

    // OPEN cannot be opened again.
    w.domain.open_meeting(&Actor::Host, draft).unwrap();
    let again = w.domain.open_meeting(&Actor::Host, draft).unwrap_err();
    assert!(
        matches!(again, DomainError::InvalidTransition { .. }),
        "{again:?}"
    );
    assert_eq!(w.audits(), 1, "the repeat must not add a second audit row");
}

#[test]
fn there_is_no_unlock() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    // The only lifecycle calls that exist are open and lock, and both refuse a
    // locked meeting. Nothing in the API can return it to OPEN.
    assert!(matches!(
        w.domain.open_meeting(&Actor::Host, meeting_id),
        Err(DomainError::MeetingLocked { .. })
    ));
    assert_eq!(w.meeting_status(meeting_id), "LOCKED");
}

#[test]
fn a_missing_meeting_is_reported_as_such() {
    let w = World::new();
    let ghost = MeetingId::new();
    let err = w.domain.open_meeting(&Actor::Host, ghost).unwrap_err();
    assert_eq!(err, DomainError::MeetingNotFound { meeting_id: ghost });
}

#[test]
fn a_note_for_a_participant_outside_the_meeting_is_rejected() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let stranger = ParticipantId::new();

    let err = w
        .domain
        .write_note(&Actor::Host, note(meeting_id, stranger, "content"))
        .unwrap_err();

    assert_eq!(
        err,
        DomainError::ParticipantNotFound {
            meeting_id,
            participant_id: stranger,
        }
    );
    assert_eq!(w.notes(), 0);
}

// ---------------------------------------------------------------------------
// 9. A locked meeting rejects every mutation this step introduces
// ---------------------------------------------------------------------------

#[test]
fn a_locked_meeting_rejects_every_mutation() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    // One note exists before the lock.
    w.domain
        .write_note(&Actor::Host, note(meeting_id, budi, "before the lock"))
        .unwrap();
    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    let before = (w.notes(), w.versions(), w.audits());

    // Every mutation the domain offers, by every actor kind.
    let attempts: Vec<DomainError> = vec![
        w.domain.open_meeting(&Actor::Host, meeting_id).unwrap_err(),
        w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap_err(),
        w.domain
            .write_note(&Actor::Host, note(meeting_id, budi, "host edit"))
            .unwrap_err(),
        w.domain
            .write_note(
                &participant_actor(meeting_id, budi),
                note(meeting_id, budi, "participant edit"),
            )
            .unwrap_err(),
        w.domain
            .write_note(
                &Actor::RemoteImport {
                    meeting_id,
                    participant_id: budi,
                },
                note(meeting_id, budi, "imported edit"),
            )
            .unwrap_err(),
    ];

    for err in &attempts {
        assert!(
            matches!(err, DomainError::MeetingLocked { .. }),
            "expected MeetingLocked, got {err:?}"
        );
        // Actionable: names the meeting the Host would recognise.
        assert!(err.to_string().contains("Weekly Coordination"));
    }

    assert_eq!(
        (w.notes(), w.versions(), w.audits()),
        before,
        "a locked meeting must leave data and audit untouched"
    );
}

// ---------------------------------------------------------------------------
// 2. A mutation racing with LOCKED
// ---------------------------------------------------------------------------

#[test]
fn a_stale_view_of_an_open_meeting_cannot_authorise_a_write() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    // A caller observes the meeting as OPEN...
    assert_eq!(w.meeting_status(meeting_id), "OPEN");

    // ...the Host locks it...
    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    // ...and the write, which re-reads status inside its own transaction,
    // refuses. This is the whole point of not trusting an earlier read.
    let err = w
        .domain
        .write_note(
            &participant_actor(meeting_id, budi),
            note(meeting_id, budi, "too late"),
        )
        .unwrap_err();

    assert!(matches!(err, DomainError::MeetingLocked { .. }), "{err:?}");
    assert_eq!(w.notes(), 0);
}

#[test]
fn writes_racing_a_lock_either_commit_before_it_or_are_refused() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    const WRITERS: usize = 16;
    let mut outcomes = Vec::new();
    // Borrow once; the threads share the one boundary rather than owning it.
    let domain = &w.domain;

    std::thread::scope(|scope| {
        let mut handles = Vec::new();

        for n in 0..WRITERS {
            handles.push(scope.spawn(move || {
                domain.write_note(
                    &participant_actor(meeting_id, budi),
                    note(meeting_id, budi, &format!("revision {n}")),
                )
            }));
        }
        let locker = scope.spawn(move || domain.lock_meeting(&Actor::Host, meeting_id));

        for handle in handles {
            outcomes.push(handle.join().expect("writer thread"));
        }
        locker.join().expect("locker thread").expect("lock");
    });

    // The lock always wins eventually: nothing can reopen the meeting.
    assert_eq!(w.meeting_status(meeting_id), "LOCKED");

    let mut committed = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(written) => committed.push(written.version),
            // The only legitimate refusal in this race.
            Err(DomainError::MeetingLocked { .. }) => {}
            Err(other) => panic!("unexpected error in lock race: {other:?}"),
        }
    }

    // Whatever the interleaving was, the database must agree exactly with the
    // successful calls: every commit produced one version row and one audit
    // row, and every refusal produced neither.
    committed.sort_unstable();
    let expected: Vec<i64> = (1..=committed.len() as i64).collect();
    assert_eq!(
        committed, expected,
        "committed versions must be a dense 1..n sequence"
    );
    assert_eq!(w.versions(), committed.len() as i64);
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action LIKE 'note.%'"),
        committed.len() as i64
    );
}

// ---------------------------------------------------------------------------
// 1. Concurrent note writes and version numbering
// ---------------------------------------------------------------------------

#[test]
fn concurrent_writes_to_one_note_cannot_produce_the_same_version() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    const WRITERS: usize = 16;
    let mut versions = Vec::new();
    let domain = &w.domain;

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for n in 0..WRITERS {
            handles.push(scope.spawn(move || {
                domain.write_note(
                    &participant_actor(meeting_id, budi),
                    note(meeting_id, budi, &format!("revision {n}")),
                )
            }));
        }
        for handle in handles {
            versions.push(handle.join().expect("writer thread").expect("write"));
        }
    });

    // Exactly one note, despite 16 concurrent writers (ADR-0003).
    assert_eq!(w.notes(), 1);

    let mut numbers: Vec<i64> = versions.iter().map(|v| v.version).collect();
    numbers.sort_unstable();
    assert_eq!(
        numbers,
        (1..=WRITERS as i64).collect::<Vec<_>>(),
        "versions must be unique and dense"
    );

    // Exactly one writer saw an empty note, so exactly one call created it.
    assert_eq!(versions.iter().filter(|v| v.created).count(), 1);

    // And the database agrees: no duplicate version survived.
    assert_eq!(w.versions(), WRITERS as i64);
    let distinct = w.count("SELECT count(DISTINCT version) FROM note_versions");
    assert_eq!(distinct, WRITERS as i64);
}

#[test]
fn a_second_write_updates_the_single_note_and_appends_history() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");
    let actor = participant_actor(meeting_id, budi);

    let first = w
        .domain
        .write_note(&actor, note(meeting_id, budi, "first draft"))
        .unwrap();
    let second = w
        .domain
        .write_note(&actor, note(meeting_id, budi, "second draft"))
        .unwrap();

    assert!(first.created && first.version == 1);
    assert!(!second.created && second.version == 2);
    assert_eq!(
        first.note_id, second.note_id,
        "the note must not be replaced"
    );

    assert_eq!(w.notes(), 1);
    assert_eq!(w.versions(), 2);

    // Current content is the latest; history keeps the earlier text.
    let content: String = w
        .db()
        .read(|conn| Ok(conn.query_row("SELECT content FROM notes", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(content, "second draft");

    let history: Vec<String> = w
        .db()
        .read(|conn| {
            let mut stmt =
                conn.prepare("SELECT content FROM note_versions ORDER BY version ASC")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(history, vec!["first draft", "second draft"]);
}

#[test]
fn a_caller_cannot_choose_a_version_number() {
    // There is no field to set: `WriteNote` carries content and a target, and
    // the version is derived inside the transaction. This test pins that shape
    // so a future change has to be deliberate.
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    let command = WriteNote {
        meeting_id,
        participant_id: budi,
        content: "only content".to_owned(),
    };
    assert_eq!(
        w.domain.write_note(&Actor::Host, command).unwrap().version,
        1
    );
}

// ---------------------------------------------------------------------------
// 6 & 7. Atomicity of mutation and audit
// ---------------------------------------------------------------------------

#[test]
fn a_successful_write_commits_note_history_and_audit_together() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");

    let written = w
        .domain
        .write_note(&Actor::Host, note(meeting_id, budi, "## Agenda"))
        .unwrap();

    assert_eq!((w.notes(), w.versions()), (1, 1));

    let (action, target_type, target_id, actor_type, metadata): (
        String,
        String,
        String,
        String,
        String,
    ) = w
        .db()
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT action, target_type, target_id, actor_type, metadata FROM audit_logs",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?)
        })
        .unwrap();

    assert_eq!(action, "note.created");
    assert_eq!(target_type, "note");
    assert_eq!(target_id, written.note_id.to_storage());
    assert_eq!(actor_type, "HOST");
    // The audit record describes the same version the caller was told about.
    assert!(metadata.contains(r#""version":1"#), "{metadata}");

    // A second write is an update, not a create.
    w.domain
        .write_note(&Actor::Host, note(meeting_id, budi, "## Agenda v2"))
        .unwrap();
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'note.updated'"),
        1
    );
}

#[test]
fn a_refused_mutation_leaves_data_and_audit_untouched() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);
    let budi = w.seed_participant(meeting_id, "Budi Santoso");
    let siti = w.seed_participant(meeting_id, "Siti");

    w.domain
        .write_note(&Actor::Host, note(meeting_id, budi, "the only note"))
        .unwrap();
    let before = (w.notes(), w.versions(), w.audits());

    // A refusal from each stage of the pipeline: authorization, validation,
    // and the participant check.
    let refusals = [
        w.domain
            .write_note(
                &participant_actor(meeting_id, budi),
                note(meeting_id, siti, "not mine"),
            )
            .unwrap_err(),
        w.domain
            .write_note(&Actor::Host, note(meeting_id, budi, "   "))
            .unwrap_err(),
        w.domain
            .write_note(&Actor::Host, note(meeting_id, ParticipantId::new(), "x"))
            .unwrap_err(),
    ];

    for err in &refusals {
        assert!(err.is_refusal(), "{err:?}");
    }

    assert_eq!(
        (w.notes(), w.versions(), w.audits()),
        before,
        "a refused mutation must not write anything, including audit"
    );

    // The existing note is untouched.
    let content: String = w
        .db()
        .read(|conn| Ok(conn.query_row("SELECT content FROM notes", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(content, "the only note");
}

#[test]
fn a_rejected_transition_writes_no_audit_record() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Draft);

    assert!(w.domain.lock_meeting(&Actor::Host, meeting_id).is_err());
    assert_eq!(w.audits(), 0);
    assert_eq!(w.meeting_status(meeting_id), "DRAFT");

    // And the successful one does write exactly one.
    w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();
    assert_eq!(w.audits(), 1);

    let (action, metadata): (String, String) = w
        .db()
        .read(|conn| {
            Ok(
                conn.query_row("SELECT action, metadata FROM audit_logs", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?,
            )
        })
        .unwrap();
    assert_eq!(action, "meeting.opened");
    assert!(
        metadata.contains("DRAFT") && metadata.contains("OPEN"),
        "{metadata}"
    );
}

#[test]
fn locking_records_the_lock_time_alongside_the_status() {
    let w = World::new();
    let meeting_id = w.seed_meeting(MeetingStatus::Open);

    let locked = w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    let (status, locked_at): (String, Option<Sql<UtcTimestamp>>) = w
        .db()
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT status, locked_at FROM meetings WHERE id = ?1",
                params![Sql(meeting_id)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();

    assert_eq!(status, "LOCKED");
    assert_eq!(locked_at.map(Sql::into_inner), Some(locked.at));
}
