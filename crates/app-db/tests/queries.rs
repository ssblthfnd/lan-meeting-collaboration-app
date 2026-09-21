//! Read-side query tests.
//!
//! The mutation tests prove the rules; these prove the other half: that what the
//! Host UI is shown is typed, complete, deterministically ordered, and produced
//! without taking the write path (ADR-0014).
//!
//! Data is created through the domain rather than seeded with SQL, so the read
//! models are checked against state a real mutation produced.

use app_core::actor::Actor;
use app_core::id::MeetingId;
use app_core::meeting::{MeetingConfiguration, MeetingStatus};
use app_core::participant::ParticipantDetails;
use app_core::service::Domain;
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone};
use app_db::query::HostQueries;
use app_db::Db;
use std::sync::Arc;
use tempfile::TempDir;

struct World {
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

    fn queries(&self) -> HostQueries<'_> {
        HostQueries::new(&self.db)
    }

    fn create(&self, title: &str, date: (i16, i8, i8), start: (i8, i8, i8)) -> MeetingId {
        self.domain
            .create_meeting(
                &Actor::Host,
                MeetingConfiguration {
                    title: title.to_owned(),
                    topic: Some("Budget".to_owned()),
                    date: MeetingDate::new(date.0, date.1, date.2).unwrap(),
                    start_time: MeetingTime::new(start.0, start.1, start.2).unwrap(),
                    end_time: MeetingTime::new(23, 59, 59).unwrap(),
                    timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
                    location: Some("Meeting Room 2".to_owned()),
                    description: None,
                },
            )
            .expect("create meeting")
            .meeting_id
    }

    fn add(&self, meeting_id: MeetingId, name: &str) {
        self.domain
            .add_participant(
                &Actor::Host,
                meeting_id,
                ParticipantDetails {
                    name: name.to_owned(),
                    department: Some("Finance".to_owned()),
                    position: None,
                    meeting_role: None,
                },
            )
            .expect("add participant");
    }
}

// ---------------------------------------------------------------------------
// Typed results
// ---------------------------------------------------------------------------

#[test]
fn a_meeting_detail_carries_every_configured_field() {
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));
    w.add(meeting_id, "Budi Santoso");

    let detail = w.queries().meeting(meeting_id).unwrap().expect("found");

    assert_eq!(detail.id, meeting_id);
    assert_eq!(detail.title, "Weekly Coordination");
    assert_eq!(detail.topic.as_deref(), Some("Budget"));
    assert_eq!(detail.location.as_deref(), Some("Meeting Room 2"));
    assert_eq!(detail.description, None);
    assert_eq!(detail.status, MeetingStatus::Draft);
    assert_eq!(detail.timezone, "Asia/Makassar");
    assert_eq!(detail.date.to_storage(), "2026-09-20");
    assert_eq!(detail.start_time.to_storage(), "09:00:00");
    // Derived, not stored: the count is a fact about the roster.
    assert_eq!(detail.participant_count, 1);
    assert_eq!(detail.locked_at, None);
    assert_eq!(detail.created_at, detail.updated_at);
}

#[test]
fn a_missing_meeting_reads_as_absent_rather_than_failing() {
    let w = World::new();
    assert_eq!(w.queries().meeting(MeetingId::new()).unwrap(), None);
}

#[test]
fn the_lock_time_appears_once_the_meeting_is_locked() {
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));
    w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();
    let locked = w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    let detail = w.queries().meeting(meeting_id).unwrap().expect("found");
    assert_eq!(detail.status, MeetingStatus::Locked);
    assert_eq!(detail.locked_at, Some(locked.at));
}

#[test]
fn a_participant_summary_reuses_the_domains_own_details_type() {
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));
    w.add(meeting_id, "Budi Santoso");

    let roster = w.queries().participants(meeting_id).unwrap();
    assert_eq!(roster.len(), 1);
    assert_eq!(
        roster[0].details,
        ParticipantDetails {
            name: "Budi Santoso".to_owned(),
            department: Some("Finance".to_owned()),
            position: None,
            meeting_role: None,
        }
    );
}

#[test]
fn a_roster_is_scoped_to_its_own_meeting() {
    let w = World::new();
    let ours = w.create("Ours", (2026, 9, 20), (9, 0, 0));
    let theirs = w.create("Theirs", (2026, 9, 21), (9, 0, 0));
    w.add(ours, "Insider");
    w.add(theirs, "Outsider");

    let roster = w.queries().participants(ours).unwrap();
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].details.name, "Insider");
}

// ---------------------------------------------------------------------------
// Deterministic ordering (architecture rules 26.4)
// ---------------------------------------------------------------------------

#[test]
fn meetings_are_ordered_by_schedule_with_the_id_as_a_total_tiebreaker() {
    let w = World::new();
    let earlier = w.create("Earlier day", (2026, 9, 19), (9, 0, 0));
    let later = w.create("Later day", (2026, 9, 21), (9, 0, 0));
    let same_day_early = w.create("Same day, 08:00", (2026, 9, 20), (8, 0, 0));
    let same_day_late = w.create("Same day, 14:00", (2026, 9, 20), (14, 0, 0));

    let listed: Vec<MeetingId> = w
        .queries()
        .meetings()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();

    // Most recent schedule first, and within a day the later start first.
    assert_eq!(listed, vec![later, same_day_late, same_day_early, earlier]);

    // Two meetings at the same date and time still have one defined order, and
    // it is stable across repeated queries.
    let twin_a = w.create("Twin", (2026, 10, 1), (9, 0, 0));
    let twin_b = w.create("Twin", (2026, 10, 1), (9, 0, 0));
    let first = w.queries().meetings().unwrap();
    for _ in 0..3 {
        let again = w.queries().meetings().unwrap();
        assert_eq!(
            again.iter().map(|m| m.id).collect::<Vec<_>>(),
            first.iter().map(|m| m.id).collect::<Vec<_>>()
        );
    }
    // The tiebreaker is the id, and UUIDv7 makes that chronological: the one
    // created second sorts first under DESC (ADR-0011).
    let twins: Vec<MeetingId> = first
        .iter()
        .filter(|m| m.title == "Twin")
        .map(|m| m.id)
        .collect();
    assert_eq!(twins, vec![twin_b, twin_a]);
}

#[test]
fn participants_are_ordered_by_name_then_id() {
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));

    for name in ["Siti Rahayu", "Ahmad Fauzi", "Budi Santoso", "Ahmad Fauzi"] {
        w.add(meeting_id, name);
    }

    let roster = w.queries().participants(meeting_id).unwrap();
    let names: Vec<&str> = roster.iter().map(|p| p.details.name.as_str()).collect();
    assert_eq!(
        names,
        ["Ahmad Fauzi", "Ahmad Fauzi", "Budi Santoso", "Siti Rahayu"]
    );

    // The two identical names are separated by id, ascending, so the order is
    // total rather than "whichever the database felt like".
    assert!(roster[0].id < roster[1].id);
}

#[test]
fn audit_entries_are_chronological_and_scoped_to_their_meeting() {
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));
    let other = w.create("Another", (2026, 9, 21), (9, 0, 0));

    w.add(meeting_id, "Budi Santoso");
    w.domain.open_meeting(&Actor::Host, meeting_id).unwrap();
    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    let entries = w.queries().audit_entries(meeting_id).unwrap();
    let actions: Vec<&str> = entries.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        [
            "meeting.created",
            "participant.added",
            "meeting.opened",
            "meeting.locked"
        ]
    );

    // Timestamps never go backwards, and ids break a tie within a millisecond.
    for pair in entries.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert!(a.created_at <= b.created_at);
        if a.created_at == b.created_at {
            assert!(a.id < b.id);
        }
    }

    // The other meeting's trail is its own.
    let theirs = w.queries().audit_entries(other).unwrap();
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0].action, "meeting.created");
}

// ---------------------------------------------------------------------------
// Audit metadata
// ---------------------------------------------------------------------------

#[test]
fn audit_metadata_is_returned_as_parsed_json() {
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));

    let entries = w.queries().audit_entries(meeting_id).unwrap();
    let metadata = entries[0].metadata.as_ref().expect("metadata present");

    // An object, not a string the caller has to re-parse.
    assert!(metadata.is_object(), "{metadata}");
    assert_eq!(metadata["title"], "Weekly Coordination");
    // The schedule is recorded with the timezone that gives it meaning.
    assert_eq!(metadata["timezone"], "Asia/Makassar");
    assert_eq!(metadata["date"], "2026-09-20");
}

#[test]
fn a_host_action_records_no_participant_as_its_actor() {
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));

    let entries = w.queries().audit_entries(meeting_id).unwrap();
    assert_eq!(entries[0].actor_type, "HOST");
    // The schema requires actor_id IS NULL exactly when the type is HOST.
    assert_eq!(entries[0].actor_id, None);
}

// ---------------------------------------------------------------------------
// Reads do not take the write path (ADR-0014)
// ---------------------------------------------------------------------------

#[test]
fn queries_run_on_read_only_connections() {
    // The read pool is opened SQLITE_OPEN_READ_ONLY, so the query path could not
    // mutate anything even if a statement tried to. Proven by attempting a write
    // through the same `Db::read` the queries use.
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));

    let attempted = w.db.read(|conn| {
        conn.execute(
            "UPDATE meetings SET title = 'renamed through a read connection'",
            [],
        )?;
        Ok(())
    });

    assert!(attempted.is_err(), "a read connection accepted a write");
    assert_eq!(
        w.queries().meeting(meeting_id).unwrap().unwrap().title,
        "Weekly Coordination"
    );
}

#[test]
fn many_readers_and_a_writer_do_not_block_each_other() {
    // WAL plus a separate read pool: a query never waits for the writer, which
    // is why the read path is worth having as its own boundary rather than
    // routing list queries through a write transaction.
    let w = World::new();
    let meeting_id = w.create("Weekly Coordination", (2026, 9, 20), (9, 0, 0));
    let (queries, domain) = (w.queries(), &w.domain);

    std::thread::scope(|scope| {
        let readers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    for _ in 0..25 {
                        queries.meetings().expect("list");
                        queries.meeting(meeting_id).expect("detail");
                        queries.participants(meeting_id).expect("roster");
                        queries.audit_entries(meeting_id).expect("audit");
                    }
                })
            })
            .collect();

        for n in 0..25 {
            domain
                .add_participant(
                    &Actor::Host,
                    meeting_id,
                    ParticipantDetails {
                        name: format!("Person {n}"),
                        department: None,
                        position: None,
                        meeting_role: None,
                    },
                )
                .expect("add");
        }

        for reader in readers {
            reader.join().expect("reader thread");
        }
    });

    assert_eq!(w.queries().participants(meeting_id).unwrap().len(), 25);
    assert_eq!(
        w.queries()
            .meeting(meeting_id)
            .unwrap()
            .unwrap()
            .participant_count,
        25
    );
}
