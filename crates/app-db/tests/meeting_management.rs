//! Meeting and participant management, against a real SQLite database.
//!
//! These exercise the operations a Host performs while preparing a meeting:
//! creating it, configuring it, building the roster, and opening it. They live in
//! `app-db` for the same reason the step-2 tests do - the rules are only
//! meaningful against real transactions, real constraint violations and real
//! concurrent writers. `app-core` keeps the tests that need no database.
//!
//! Nothing here mocks a transaction or a lock. Concurrency is tested with actual
//! threads against one database, and there are no sleeps to tune.

use app_core::actor::Actor;
use app_core::error::DomainError;
use app_core::id::{MeetingId, ParticipantId, SessionId};
use app_core::meeting::{MeetingConfiguration, MeetingStatus};
use app_core::participant::{ParticipantDetails, MAX_PARTICIPANTS};
use app_core::port::{Database, DomainTx};
use app_core::service::Domain;
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone, UtcTimestamp};
use app_db::{Db, Sql};
use rusqlite::params;
use std::sync::Arc;
use tempfile::TempDir;

/// A migrated database plus the domain boundary wrapped around it.
///
/// Unlike the step-2 fixture, nothing here seeds a meeting with raw SQL: the
/// meeting-creation flow now exists, so the tests use it.
struct World {
    db: Arc<Db>,
    domain: Domain<Arc<Db>>,
    _dir: TempDir,
}

impl World {
    fn new() -> Self {
        let (db, dir) = fresh_database();
        World {
            domain: Domain::new(Arc::clone(&db)),
            db,
            _dir: dir,
        }
    }

    fn db(&self) -> &Db {
        &self.db
    }

    /// A meeting in `DRAFT`, created through the domain boundary.
    fn draft(&self) -> MeetingId {
        self.domain
            .create_meeting(&Actor::Host, configuration())
            .expect("create meeting")
            .meeting_id
    }

    /// A meeting in `OPEN`, reached the only way there is.
    fn open(&self) -> MeetingId {
        let meeting_id = self.draft();
        self.domain
            .open_meeting(&Actor::Host, meeting_id)
            .expect("open meeting");
        meeting_id
    }

    /// A meeting in `LOCKED`.
    fn locked(&self) -> MeetingId {
        let meeting_id = self.open();
        self.domain
            .lock_meeting(&Actor::Host, meeting_id)
            .expect("lock meeting");
        meeting_id
    }

    fn add(&self, meeting_id: MeetingId, name: &str) -> ParticipantId {
        self.domain
            .add_participant(&Actor::Host, meeting_id, details(name))
            .expect("add participant")
            .participant_id
    }

    fn count(&self, sql: &str) -> i64 {
        self.db()
            .read(|conn| Ok(conn.query_row(sql, [], |row| row.get(0))?))
            .expect("count")
    }

    fn participants(&self) -> i64 {
        self.count("SELECT count(*) FROM participants")
    }

    fn audits(&self) -> i64 {
        self.count("SELECT count(*) FROM audit_logs")
    }

    /// Audit actions in their explicit total order (architecture rules 26.4).
    fn audit_actions(&self) -> Vec<String> {
        self.db()
            .read(|conn| {
                let mut stmt =
                    conn.prepare("SELECT action FROM audit_logs ORDER BY created_at ASC, id ASC")?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .expect("audit actions")
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

    fn stored_configuration(&self, meeting_id: MeetingId) -> StoredConfiguration {
        self.db()
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT id, title, topic, date, start_time, end_time, timezone,
                            location, description, status, join_token_hash,
                            created_at, updated_at, locked_at
                       FROM meetings WHERE id = ?1",
                    params![Sql(meeting_id)],
                    |row| {
                        Ok(StoredConfiguration {
                            id: row.get::<_, Sql<MeetingId>>(0)?.into_inner(),
                            title: row.get(1)?,
                            topic: row.get(2)?,
                            date: row.get::<_, Sql<MeetingDate>>(3)?.into_inner(),
                            start_time: row.get::<_, Sql<MeetingTime>>(4)?.into_inner(),
                            end_time: row.get::<_, Sql<MeetingTime>>(5)?.into_inner(),
                            timezone: row.get(6)?,
                            location: row.get(7)?,
                            description: row.get(8)?,
                            status: row.get(9)?,
                            join_token_hash: row.get(10)?,
                            created_at: row.get::<_, Sql<UtcTimestamp>>(11)?.into_inner(),
                            updated_at: row.get::<_, Sql<UtcTimestamp>>(12)?.into_inner(),
                            locked_at: row
                                .get::<_, Option<Sql<UtcTimestamp>>>(13)?
                                .map(Sql::into_inner),
                        })
                    },
                )?)
            })
            .expect("stored configuration")
    }

    fn stored_participant(&self, participant_id: ParticipantId) -> ParticipantDetails {
        self.db()
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT name, department, position, meeting_role
                       FROM participants WHERE id = ?1",
                    params![Sql(participant_id)],
                    |row| {
                        Ok(ParticipantDetails {
                            name: row.get(0)?,
                            department: row.get(1)?,
                            position: row.get(2)?,
                            meeting_role: row.get(3)?,
                        })
                    },
                )?)
            })
            .expect("stored participant")
    }
}

/// Every column of `meetings`, read back exactly as stored.
struct StoredConfiguration {
    id: MeetingId,
    title: String,
    topic: Option<String>,
    date: MeetingDate,
    start_time: MeetingTime,
    end_time: MeetingTime,
    timezone: String,
    location: Option<String>,
    description: Option<String>,
    status: String,
    join_token_hash: Option<String>,
    created_at: UtcTimestamp,
    updated_at: UtcTimestamp,
    locked_at: Option<UtcTimestamp>,
}

fn fresh_database() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open database"));
    (db, dir)
}

fn configuration() -> MeetingConfiguration {
    MeetingConfiguration {
        title: "Weekly Coordination".to_owned(),
        topic: Some("Budget".to_owned()),
        date: MeetingDate::new(2026, 9, 20).unwrap(),
        start_time: MeetingTime::new(9, 0, 0).unwrap(),
        end_time: MeetingTime::new(10, 30, 0).unwrap(),
        timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
        location: Some("Meeting Room 2".to_owned()),
        description: Some("Coordination of weekly deliverables.".to_owned()),
    }
}

fn details(name: &str) -> ParticipantDetails {
    ParticipantDetails {
        name: name.to_owned(),
        department: Some("Finance".to_owned()),
        position: Some("Analyst".to_owned()),
        meeting_role: Some("Note taker".to_owned()),
    }
}

fn participant_actor(meeting_id: MeetingId, participant_id: ParticipantId) -> Actor {
    Actor::Participant {
        meeting_id,
        participant_id,
        session_id: SessionId::new(),
    }
}

// ---------------------------------------------------------------------------
// 1 & 2 & 21. Creation
// ---------------------------------------------------------------------------

#[test]
fn a_created_meeting_starts_in_draft() {
    let w = World::new();

    let created = w
        .domain
        .create_meeting(&Actor::Host, configuration())
        .unwrap();

    assert_eq!(created.status, MeetingStatus::Draft);
    assert_eq!(w.meeting_status(created.meeting_id), "DRAFT");
    // A new meeting has no roster and is not locked.
    assert_eq!(w.participants(), 0);
    assert_eq!(w.stored_configuration(created.meeting_id).locked_at, None);
}

#[test]
fn creation_persists_every_configured_field_and_nothing_else() {
    let w = World::new();
    let input = configuration();

    let created = w
        .domain
        .create_meeting(&Actor::Host, input.clone())
        .unwrap();
    let stored = w.stored_configuration(created.meeting_id);

    assert_eq!(stored.id, created.meeting_id);
    assert_eq!(stored.title, input.title);
    assert_eq!(stored.topic, input.topic);
    assert_eq!(stored.date, input.date);
    assert_eq!(stored.start_time, input.start_time);
    assert_eq!(stored.end_time, input.end_time);
    // The timezone is stored as the IANA identifier itself, not as an offset.
    assert_eq!(stored.timezone, "Asia/Makassar");
    assert_eq!(stored.location, input.location);
    assert_eq!(stored.description, input.description);
    assert_eq!(stored.status, "DRAFT");

    // A join token belongs to the LAN server's lifecycle, not to creation.
    assert_eq!(stored.join_token_hash, None);
}

#[test]
fn ids_and_timestamps_round_trip_through_persistence() {
    let w = World::new();

    let created = w
        .domain
        .create_meeting(&Actor::Host, configuration())
        .unwrap();
    let stored = w.stored_configuration(created.meeting_id);

    // The id survives as the same UUIDv7 it was minted as, in canonical form.
    assert_eq!(stored.id, created.meeting_id);
    assert_eq!(
        w.db()
            .read(|conn| Ok(conn.query_row(
                "SELECT id FROM meetings WHERE id = ?1",
                params![Sql(created.meeting_id)],
                |row| row.get::<_, String>(0)
            )?))
            .unwrap(),
        created.meeting_id.to_storage()
    );

    // The timestamp survives to the millisecond, and creation sets both
    // created_at and updated_at to the same instant.
    assert_eq!(stored.created_at, created.at);
    assert_eq!(stored.updated_at, created.at);

    // The zoneless schedule survives as written: 09:00 stays 09:00, because it
    // is read in the meeting's timezone rather than converted (PRD 25.2).
    assert_eq!(stored.date.to_storage(), "2026-09-20");
    assert_eq!(stored.start_time.to_storage(), "09:00:00");

    // A participant id round-trips the same way.
    let budi = w.add(created.meeting_id, "Budi Santoso");
    let read_back: Sql<ParticipantId> = w
        .db()
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT id FROM participants WHERE id = ?1",
                params![Sql(budi)],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(read_back.into_inner(), budi);

    // And so does the update timestamp of a later configuration change.
    let updated = w
        .domain
        .update_meeting(&Actor::Host, created.meeting_id, configuration())
        .unwrap();
    let after = w.stored_configuration(created.meeting_id);
    assert_eq!(after.updated_at, updated.at);
    assert_eq!(
        after.created_at, created.at,
        "created_at must not move on update"
    );
}

#[test]
fn an_invalid_configuration_creates_no_meeting() {
    let w = World::new();

    let mut blank_title = configuration();
    blank_title.title = "   ".to_owned();
    let err = w
        .domain
        .create_meeting(&Actor::Host, blank_title)
        .unwrap_err();
    assert!(
        matches!(
            err,
            DomainError::Validation {
                field: "meeting title",
                ..
            }
        ),
        "{err:?}"
    );

    let mut backwards = configuration();
    backwards.start_time = MeetingTime::new(11, 0, 0).unwrap();
    backwards.end_time = MeetingTime::new(10, 0, 0).unwrap();
    assert!(w.domain.create_meeting(&Actor::Host, backwards).is_err());

    assert_eq!(w.count("SELECT count(*) FROM meetings"), 0);
    assert_eq!(w.audits(), 0);
}

// ---------------------------------------------------------------------------
// 3 - 8. Configuration mutability and the lifecycle
// ---------------------------------------------------------------------------

#[test]
fn a_host_can_edit_a_draft_meeting() {
    let w = World::new();
    let meeting_id = w.draft();

    let mut revised = configuration();
    revised.title = "Weekly Coordination (revised)".to_owned();
    revised.start_time = MeetingTime::new(13, 0, 0).unwrap();
    revised.end_time = MeetingTime::new(14, 0, 0).unwrap();
    revised.timezone = MeetingTimeZone::new("Asia/Jayapura").unwrap();
    revised.location = None;

    w.domain
        .update_meeting(&Actor::Host, meeting_id, revised.clone())
        .unwrap();

    let stored = w.stored_configuration(meeting_id);
    assert_eq!(stored.title, revised.title);
    assert_eq!(stored.start_time, revised.start_time);
    assert_eq!(stored.timezone, "Asia/Jayapura");
    assert_eq!(stored.location, None);
    // The lifecycle is untouched by a configuration change.
    assert_eq!(stored.status, "DRAFT");
    assert_eq!(stored.locked_at, None);
}

#[test]
fn a_participant_cannot_edit_meeting_configuration() {
    let w = World::new();
    let meeting_id = w.draft();
    let budi = w.add(meeting_id, "Budi Santoso");
    let before = w.stored_configuration(meeting_id).title;

    let mut theirs = configuration();
    theirs.title = "Renamed by a participant".to_owned();

    for actor in [
        participant_actor(meeting_id, budi),
        Actor::RemoteImport {
            meeting_id,
            participant_id: budi,
        },
    ] {
        let err = w
            .domain
            .update_meeting(&actor, meeting_id, theirs.clone())
            .unwrap_err();
        assert!(matches!(err, DomainError::Forbidden { .. }), "{err:?}");
    }

    assert_eq!(w.stored_configuration(meeting_id).title, before);
}

#[test]
fn meeting_configuration_cannot_be_edited_once_open() {
    let w = World::new();
    let meeting_id = w.open();
    let before = w.stored_configuration(meeting_id).title;
    let audits_before = w.audits();

    let mut revised = configuration();
    revised.title = "Too late".to_owned();

    let err = w
        .domain
        .update_meeting(&Actor::Host, meeting_id, revised)
        .unwrap_err();

    assert_eq!(
        err,
        DomainError::MeetingNotDraft {
            meeting_id,
            detected: MeetingStatus::Open,
        }
    );
    // Actionable: names the expected and the detected status.
    let message = err.to_string();
    assert!(
        message.contains("DRAFT") && message.contains("OPEN"),
        "{message}"
    );

    assert_eq!(w.stored_configuration(meeting_id).title, before);
    assert_eq!(w.audits(), audits_before);
}

#[test]
fn meeting_configuration_cannot_be_edited_once_locked() {
    let w = World::new();
    let meeting_id = w.locked();
    let before = w.stored_configuration(meeting_id).title;
    let audits_before = w.audits();

    let mut revised = configuration();
    revised.title = "Too late".to_owned();

    let err = w
        .domain
        .update_meeting(&Actor::Host, meeting_id, revised)
        .unwrap_err();

    // A locked meeting is the stronger refusal: nothing changes at all, which
    // is a different fact from "configuration is settled".
    assert!(matches!(err, DomainError::MeetingLocked { .. }), "{err:?}");
    assert!(err.to_string().contains("Weekly Coordination"));

    assert_eq!(w.stored_configuration(meeting_id).title, before);
    assert_eq!(w.audits(), audits_before);
}

#[test]
fn a_draft_meeting_opens_and_invalid_transitions_stay_rejected() {
    let w = World::new();
    let meeting_id = w.draft();

    // DRAFT cannot skip to LOCKED.
    let err = w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap_err();
    assert_eq!(
        err,
        DomainError::InvalidTransition {
            expected: "OPEN",
            detected: MeetingStatus::Draft,
        }
    );

    let opened = w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();
    assert_eq!(opened.from, MeetingStatus::Draft);
    assert_eq!(opened.to, MeetingStatus::Open);
    assert_eq!(w.meeting_status(meeting_id), "OPEN");

    // An open meeting cannot be opened again, and a locked one never reopens.
    assert!(w.domain.open_meeting(&Actor::Host, meeting_id).is_err());
    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();
    assert!(matches!(
        w.domain.open_meeting(&Actor::Host, meeting_id),
        Err(DomainError::MeetingLocked { .. })
    ));
    assert_eq!(w.meeting_status(meeting_id), "LOCKED");
}

// ---------------------------------------------------------------------------
// 9 - 12. The roster
// ---------------------------------------------------------------------------

#[test]
fn a_participant_can_be_added_while_the_meeting_is_draft() {
    let w = World::new();
    let meeting_id = w.draft();

    let added = w
        .domain
        .add_participant(&Actor::Host, meeting_id, details("Budi Santoso"))
        .unwrap();

    assert_eq!(added.roster_size, 1);
    assert_eq!(w.participants(), 1);
    assert_eq!(
        w.stored_participant(added.participant_id),
        details("Budi Santoso")
    );

    // Two participants may share a name: identity is the id, never the name.
    let second = w
        .domain
        .add_participant(&Actor::Host, meeting_id, details("Budi Santoso"))
        .unwrap();
    assert_ne!(second.participant_id, added.participant_id);
    assert_eq!(second.roster_size, 2);
}

#[test]
fn participant_information_can_be_edited_while_the_meeting_is_draft() {
    let w = World::new();
    let meeting_id = w.draft();
    let budi = w.add(meeting_id, "Budi Santoso");

    let revised = ParticipantDetails {
        name: "Budi Santoso".to_owned(),
        department: Some("Operations".to_owned()),
        position: None,
        meeting_role: Some("Chair".to_owned()),
    };
    w.domain
        .update_participant(&Actor::Host, meeting_id, budi, revised.clone())
        .unwrap();

    assert_eq!(w.stored_participant(budi), revised);
    // An edit is not an insert.
    assert_eq!(w.participants(), 1);
}

#[test]
fn a_participant_can_be_removed_while_the_meeting_is_draft() {
    let w = World::new();
    let meeting_id = w.draft();
    let budi = w.add(meeting_id, "Budi Santoso");
    let siti = w.add(meeting_id, "Siti");

    let removed = w
        .domain
        .remove_participant(&Actor::Host, meeting_id, budi)
        .unwrap();

    assert_eq!(removed.roster_size, 1);
    assert_eq!(w.participants(), 1);
    assert_eq!(
        w.count("SELECT count(*) FROM participants WHERE id = (SELECT id FROM participants)"),
        1
    );
    assert_eq!(w.stored_participant(siti).name, "Siti");

    // Removing them again is a refusal, not a silent success.
    let err = w
        .domain
        .remove_participant(&Actor::Host, meeting_id, budi)
        .unwrap_err();
    assert_eq!(
        err,
        DomainError::ParticipantNotFound {
            meeting_id,
            participant_id: budi,
        }
    );
}

#[test]
fn roster_mutations_are_rejected_once_the_meeting_is_open() {
    let w = World::new();
    let meeting_id = w.draft();
    let budi = w.add(meeting_id, "Budi Santoso");
    w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();

    let before = (w.participants(), w.audits());

    let refusals = [
        w.domain
            .add_participant(&Actor::Host, meeting_id, details("Late arrival"))
            .unwrap_err(),
        w.domain
            .update_participant(&Actor::Host, meeting_id, budi, details("Renamed"))
            .unwrap_err(),
        w.domain
            .remove_participant(&Actor::Host, meeting_id, budi)
            .unwrap_err(),
    ];

    for err in &refusals {
        assert_eq!(
            err,
            &DomainError::MeetingNotDraft {
                meeting_id,
                detected: MeetingStatus::Open,
            }
        );
    }

    assert_eq!((w.participants(), w.audits()), before);
}

#[test]
fn roster_mutations_are_rejected_once_the_meeting_is_locked() {
    let w = World::new();
    let meeting_id = w.draft();
    let budi = w.add(meeting_id, "Budi Santoso");
    w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();
    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    let before = (w.participants(), w.audits());

    let refusals = [
        w.domain
            .add_participant(&Actor::Host, meeting_id, details("Late arrival"))
            .unwrap_err(),
        w.domain
            .update_participant(&Actor::Host, meeting_id, budi, details("Renamed"))
            .unwrap_err(),
        w.domain
            .remove_participant(&Actor::Host, meeting_id, budi)
            .unwrap_err(),
    ];

    for err in &refusals {
        assert!(matches!(err, DomainError::MeetingLocked { .. }), "{err:?}");
    }

    assert_eq!((w.participants(), w.audits()), before);
}

#[test]
fn a_roster_mutation_for_an_unknown_meeting_is_reported_as_such() {
    let w = World::new();
    let ghost = MeetingId::new();

    let err = w
        .domain
        .add_participant(&Actor::Host, ghost, details("Nobody"))
        .unwrap_err();
    assert_eq!(err, DomainError::MeetingNotFound { meeting_id: ghost });
}

// ---------------------------------------------------------------------------
// 13 - 15. The 99-participant limit
// ---------------------------------------------------------------------------

#[test]
fn ninety_nine_participants_can_be_added_and_the_hundredth_cannot() {
    let w = World::new();
    let meeting_id = w.draft();

    for n in 1..=MAX_PARTICIPANTS {
        let added = w
            .domain
            .add_participant(&Actor::Host, meeting_id, details(&format!("Person {n}")))
            .unwrap_or_else(|e| panic!("participant {n} should be allowed: {e}"));
        assert_eq!(added.roster_size, n as i64);
    }
    assert_eq!(w.participants(), 99);

    let err = w
        .domain
        .add_participant(&Actor::Host, meeting_id, details("Person 100"))
        .unwrap_err();
    assert!(
        matches!(
            err,
            DomainError::Validation {
                field: "participant count",
                ..
            }
        ),
        "{err:?}"
    );
    // Actionable: names the limit and what was found.
    let message = err.to_string();
    assert!(message.contains("at most 99"), "{message}");
    assert!(message.contains("already has 99"), "{message}");

    assert_eq!(w.participants(), 99);
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'participant.added'"),
        99,
        "the refused addition must not be audited"
    );

    // The limit is per meeting, not global.
    let other = w.draft();
    assert_eq!(w.add(other, "Someone else").to_storage().len(), 36);
    assert_eq!(w.participants(), 100);
}

#[test]
fn the_limit_holds_after_a_removal_frees_a_place() {
    let w = World::new();
    let meeting_id = w.draft();
    let mut ids = Vec::new();
    for n in 1..=MAX_PARTICIPANTS {
        ids.push(w.add(meeting_id, &format!("Person {n}")));
    }

    let removed = w
        .domain
        .remove_participant(&Actor::Host, meeting_id, ids[0])
        .unwrap();
    assert_eq!(removed.roster_size, 98);

    // One place free, so exactly one more may join - and then no more.
    assert_eq!(
        w.domain
            .add_participant(&Actor::Host, meeting_id, details("Replacement"))
            .unwrap()
            .roster_size,
        99
    );
    assert!(w
        .domain
        .add_participant(&Actor::Host, meeting_id, details("One too many"))
        .is_err());
}

#[test]
fn concurrent_additions_cannot_commit_more_than_ninety_nine() {
    let w = World::new();
    let meeting_id = w.draft();

    // Enough writers to overrun the limit if the count were read outside the
    // transaction, or cached, or trusted from a caller.
    const WRITERS: usize = 120;
    let mut outcomes = Vec::new();
    let domain = &w.domain;

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for n in 0..WRITERS {
            handles.push(scope.spawn(move || {
                domain.add_participant(&Actor::Host, meeting_id, details(&format!("Person {n}")))
            }));
        }
        for handle in handles {
            outcomes.push(handle.join().expect("writer thread"));
        }
    });

    let mut sizes = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(added) => sizes.push(added.roster_size),
            // The only legitimate refusal in this race.
            Err(DomainError::Validation {
                field: "participant count",
                ..
            }) => {}
            Err(other) => panic!("unexpected error while racing the limit: {other:?}"),
        }
    }

    // Whatever the interleaving was: exactly 99 committed, each reporting a
    // distinct roster size in a dense 1..99 sequence, and the database agrees.
    sizes.sort_unstable();
    assert_eq!(sizes, (1..=99).collect::<Vec<i64>>());
    assert_eq!(w.participants(), 99);
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'participant.added'"),
        99
    );
}

#[test]
fn the_database_refuses_a_hundredth_participant_on_its_own() {
    // Bypasses the domain entirely and writes through raw SQL, which is the
    // point: the limit must not depend on Rust having been asked politely
    // (ADR-0011).
    let (db, _dir) = fresh_database();
    let domain = Domain::new(Arc::clone(&db));
    let meeting_id = domain
        .create_meeting(&Actor::Host, configuration())
        .unwrap()
        .meeting_id;

    let insert = |n: usize| {
        db.write(|tx| {
            tx.execute(
                "INSERT INTO participants (id, meeting_id, name, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    Sql(ParticipantId::new()),
                    Sql(meeting_id),
                    format!("Person {n}"),
                    Sql(UtcTimestamp::now()),
                ],
            )?;
            Ok(())
        })
    };

    for n in 1..=99 {
        insert(n).unwrap_or_else(|e| panic!("participant {n} should be allowed: {e}"));
    }

    let hundredth = insert(100);
    assert!(hundredth.is_err(), "a hundredth participant was accepted");
    assert!(hundredth
        .unwrap_err()
        .to_string()
        .contains("at most 99 participants"));
}

// ---------------------------------------------------------------------------
// 16. Actors acting outside their meeting
// ---------------------------------------------------------------------------

#[test]
fn an_actor_from_another_meeting_cannot_touch_this_one() {
    let w = World::new();
    let theirs = w.draft();
    let ours = w.draft();
    let outsider = w.add(theirs, "Outsider");
    let insider = w.add(ours, "Insider");

    let actor = participant_actor(theirs, outsider);

    // Not a permissions question: a session binds to one meeting (ADR-0002), so
    // the actor has no standing in another at all.
    for err in [
        w.domain
            .add_participant(&actor, ours, details("Injected"))
            .unwrap_err(),
        w.domain
            .update_participant(&actor, ours, insider, details("Renamed"))
            .unwrap_err(),
        w.domain
            .remove_participant(&actor, ours, insider)
            .unwrap_err(),
        w.domain
            .update_meeting(&actor, ours, configuration())
            .unwrap_err(),
    ] {
        assert_eq!(err, DomainError::Unauthorized { meeting_id: ours });
    }

    // And within their own meeting the same actor is forbidden rather than
    // unauthorized: they are a party to it, but the roster is not theirs.
    let err = w
        .domain
        .add_participant(&actor, theirs, details("Injected"))
        .unwrap_err();
    assert!(matches!(err, DomainError::Forbidden { .. }), "{err:?}");

    assert_eq!(w.participants(), 2);
    // Only the two additions this test performed itself are audited.
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'participant.added'"),
        2
    );
}

#[test]
fn a_participant_id_from_another_roster_is_not_reachable_through_this_meeting() {
    let w = World::new();
    let theirs = w.draft();
    let ours = w.draft();
    let outsider = w.add(theirs, "Outsider");

    // The Host is authorized here, so this is purely about scope: an id from
    // another roster is absent from this one, not editable through it.
    let err = w
        .domain
        .update_participant(&Actor::Host, ours, outsider, details("Renamed"))
        .unwrap_err();
    assert_eq!(
        err,
        DomainError::ParticipantNotFound {
            meeting_id: ours,
            participant_id: outsider,
        }
    );
    assert_eq!(w.stored_participant(outsider).name, "Outsider");
}

// ---------------------------------------------------------------------------
// 17 & 18. Audit
// ---------------------------------------------------------------------------

#[test]
fn each_meeting_mutation_writes_exactly_its_own_audit_event() {
    let w = World::new();

    let created = w
        .domain
        .create_meeting(&Actor::Host, configuration())
        .unwrap();
    assert_eq!(w.audit_actions(), vec!["meeting.created"]);

    w.domain
        .update_meeting(&Actor::Host, created.meeting_id, configuration())
        .unwrap();
    w.domain
        .open_meeting(&Actor::Host, created.meeting_id)
        .unwrap();
    w.domain
        .lock_meeting(&Actor::Host, created.meeting_id)
        .unwrap();

    assert_eq!(
        w.audit_actions(),
        vec![
            "meeting.created",
            "meeting.updated",
            "meeting.opened",
            "meeting.locked",
        ]
    );

    let (target_type, target_id, actor_type, actor_id, metadata): (
        String,
        String,
        String,
        Option<String>,
        String,
    ) = w
        .db()
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT target_type, target_id, actor_type, actor_id, metadata
                   FROM audit_logs WHERE action = 'meeting.created'",
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

    assert_eq!(target_type, "meeting");
    assert_eq!(target_id, created.meeting_id.to_storage());
    // Attribution comes from the authorization proof, never from a caller.
    assert_eq!(actor_type, "HOST");
    assert_eq!(actor_id, None);
    // The schedule is recorded with the timezone that gives it meaning.
    assert!(
        metadata.contains(r#""timezone":"Asia/Makassar""#),
        "{metadata}"
    );
    assert!(metadata.contains(r#""date":"2026-09-20""#), "{metadata}");
    assert!(metadata.contains("Weekly Coordination"), "{metadata}");
}

#[test]
fn each_participant_mutation_writes_exactly_its_own_audit_event() {
    let w = World::new();
    let meeting_id = w.draft();

    let added = w
        .domain
        .add_participant(&Actor::Host, meeting_id, details("Budi Santoso"))
        .unwrap();
    w.domain
        .update_participant(
            &Actor::Host,
            meeting_id,
            added.participant_id,
            ParticipantDetails {
                name: "Budi Santoso".to_owned(),
                department: Some("Operations".to_owned()),
                position: Some("Analyst".to_owned()),
                meeting_role: Some("Note taker".to_owned()),
            },
        )
        .unwrap();
    w.domain
        .remove_participant(&Actor::Host, meeting_id, added.participant_id)
        .unwrap();

    assert_eq!(
        w.audit_actions(),
        vec![
            "meeting.created",
            "participant.added",
            "participant.updated",
            "participant.removed",
        ]
    );

    let rows: Vec<(String, String, String, String)> = w
        .db()
        .read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT action, target_type, target_id, metadata
                   FROM audit_logs
                  WHERE action LIKE 'participant.%'
                  ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();

    for (action, target_type, target_id, _) in &rows {
        assert_eq!(target_type, "participant", "{action}");
        assert_eq!(target_id, &added.participant_id.to_storage(), "{action}");
    }

    // The update records both sides: a changed field is only meaningful next to
    // what it replaced.
    let update_metadata = &rows[1].3;
    assert!(update_metadata.contains(r#""from""#), "{update_metadata}");
    assert!(update_metadata.contains("Finance"), "{update_metadata}");
    assert!(update_metadata.contains("Operations"), "{update_metadata}");

    // The removal keeps the participant's details, because the row is gone and
    // `target_id` is deliberately not a foreign key (ADR-0011).
    let removal_metadata = &rows[2].3;
    assert!(
        removal_metadata.contains("Budi Santoso"),
        "{removal_metadata}"
    );
    assert!(
        removal_metadata.contains(r#""roster_size":0"#),
        "{removal_metadata}"
    );
    assert_eq!(w.participants(), 0);
}

// ---------------------------------------------------------------------------
// 19 & 20. Atomicity
// ---------------------------------------------------------------------------

#[test]
fn a_failed_mutation_leaves_entity_and_audit_state_unchanged() {
    let w = World::new();
    let meeting_id = w.draft();
    let budi = w.add(meeting_id, "Budi Santoso");

    let before_details = w.stored_participant(budi);
    let before_title = w.stored_configuration(meeting_id).title;
    let before = (w.participants(), w.audits());

    // A refusal from each stage of the pipeline: authorization, the lifecycle
    // rule, input validation, and the participant lookup.
    let refusals = [
        w.domain
            .update_participant(
                &participant_actor(meeting_id, budi),
                meeting_id,
                budi,
                details("Renamed"),
            )
            .unwrap_err(),
        w.domain
            .update_participant(
                &Actor::Host,
                meeting_id,
                budi,
                ParticipantDetails {
                    name: "   ".to_owned(),
                    department: None,
                    position: None,
                    meeting_role: None,
                },
            )
            .unwrap_err(),
        w.domain
            .update_participant(
                &Actor::Host,
                meeting_id,
                ParticipantId::new(),
                details("Ghost"),
            )
            .unwrap_err(),
        w.domain
            .remove_participant(&Actor::Host, meeting_id, ParticipantId::new())
            .unwrap_err(),
    ];

    for err in &refusals {
        assert!(err.is_refusal(), "{err:?}");
    }

    assert_eq!((w.participants(), w.audits()), before);
    assert_eq!(w.stored_participant(budi), before_details);
    assert_eq!(w.stored_configuration(meeting_id).title, before_title);
}

/// A database whose transaction always fails *after* the domain pipeline has
/// run to completion inside it.
///
/// This is the only way to exercise the case where the entity write and the
/// audit row were both already inserted and the transaction then rolls back.
/// Wrapping [`Database`] rather than mocking it keeps the real SQLite semantics:
/// the rollback is SQLite's, not a test double's.
struct FailsBeforeCommit(Arc<Db>);

impl Database for FailsBeforeCommit {
    fn transaction(
        &self,
        f: &mut dyn FnMut(&dyn DomainTx) -> Result<(), DomainError>,
    ) -> Result<(), DomainError> {
        self.0.transaction(&mut |tx| {
            f(tx)?;
            Err(DomainError::Persistence {
                operation: "an injected failure after the pipeline completed",
                detail: "the test forces a rollback here".to_owned(),
            })
        })
    }
}

#[test]
fn a_rollback_takes_the_mutation_and_its_audit_record_together() {
    let (db, _dir) = fresh_database();

    // A meeting and a participant, committed normally.
    let honest = Domain::new(Arc::clone(&db));
    let meeting_id = honest
        .create_meeting(&Actor::Host, configuration())
        .unwrap()
        .meeting_id;
    let budi = honest
        .add_participant(&Actor::Host, meeting_id, details("Budi Santoso"))
        .unwrap()
        .participant_id;

    let count = |sql: &str| -> i64 {
        db.read(|conn| Ok(conn.query_row(sql, [], |row| row.get(0))?))
            .unwrap()
    };
    let before = (
        count("SELECT count(*) FROM participants"),
        count("SELECT count(*) FROM audit_logs"),
        count("SELECT count(*) FROM meetings"),
    );

    let doomed = Domain::new(FailsBeforeCommit(Arc::clone(&db)));

    // Every mutation this step introduces, each of which had written its entity
    // change *and* its audit row before the transaction failed.
    for outcome in [
        doomed
            .create_meeting(&Actor::Host, configuration())
            .map(|_| ()),
        doomed
            .update_meeting(&Actor::Host, meeting_id, configuration())
            .map(|_| ()),
        doomed
            .add_participant(&Actor::Host, meeting_id, details("Siti"))
            .map(|_| ()),
        doomed
            .update_participant(&Actor::Host, meeting_id, budi, details("Renamed"))
            .map(|_| ()),
        doomed
            .remove_participant(&Actor::Host, meeting_id, budi)
            .map(|_| ()),
    ] {
        let err = outcome.unwrap_err();
        assert!(matches!(err, DomainError::Persistence { .. }), "{err:?}");
    }

    assert_eq!(
        (
            count("SELECT count(*) FROM participants"),
            count("SELECT count(*) FROM audit_logs"),
            count("SELECT count(*) FROM meetings"),
        ),
        before,
        "a rolled-back transaction must leave neither the change nor its audit record"
    );

    // Specifically: the removed participant is still there, and nothing claims
    // they were removed.
    assert_eq!(
        count(
            "SELECT count(*) FROM audit_logs WHERE action LIKE 'participant.%' \
               AND action <> 'participant.added'"
        ),
        0
    );
    assert_eq!(
        db.read(|conn| {
            Ok(conn.query_row(
                "SELECT name FROM participants WHERE id = ?1",
                params![Sql(budi)],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap(),
        "Budi Santoso"
    );
}
