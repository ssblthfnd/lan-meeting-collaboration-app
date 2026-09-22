//! The Host's note read path, against a real database.
//!
//! Reads do not go through the mutation boundary (ADR-0014), so what these
//! assert is that the four queries answer the questions the Host UI asks, with
//! the ordering the architecture rules require and the scoping that stops one
//! meeting's ids reaching another's rows.
//!
//! Every note here is written through `Domain::write_note`, never seeded, so
//! the history and authorship under test are the ones the real write path
//! produces.

use std::sync::Arc;

use app_core::actor::Actor;
use app_core::id::{MeetingId, ParticipantId};
use app_core::meeting::MeetingConfiguration;
use app_core::participant::ParticipantDetails;
use app_core::service::{Domain, WriteNote};
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone};
use app_db::participant_query::ParticipantQueries;
use app_db::query::HostQueries;
use app_db::Db;
use tempfile::TempDir;

struct World {
    db: Arc<Db>,
    domain: Domain<Arc<Db>>,
    _dir: TempDir,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open"));
        World {
            domain: Domain::new(Arc::clone(&db)),
            db,
            _dir: dir,
        }
    }

    fn queries(&self) -> HostQueries<'_> {
        HostQueries::new(&self.db)
    }

    fn meeting(&self, title: &str) -> MeetingId {
        self.domain
            .create_meeting(
                &Actor::Host,
                MeetingConfiguration {
                    title: title.to_owned(),
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

    fn write(&self, meeting_id: MeetingId, participant_id: ParticipantId, content: &str) -> i64 {
        self.domain
            .write_note(
                &Actor::Host,
                WriteNote {
                    meeting_id,
                    participant_id,
                    content: content.to_owned(),
                },
            )
            .expect("write")
            .version
    }

    /// Write as the participant themselves, through the LAN actor.
    fn write_as_participant(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        content: &str,
    ) {
        self.domain
            .write_note(
                &Actor::Participant {
                    meeting_id,
                    participant_id,
                    session_id: app_core::id::SessionId::new(),
                },
                WriteNote {
                    meeting_id,
                    participant_id,
                    content: content.to_owned(),
                },
            )
            .expect("write");
    }
}

// ---------------------------------------------------------------------------
// The current note
// ---------------------------------------------------------------------------

#[test]
fn a_participant_without_a_note_reads_as_absent() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    assert_eq!(
        world.queries().note(meeting_id, participant_id).unwrap(),
        None
    );
}

#[test]
fn a_written_note_reads_back_with_its_version_and_author() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    world.write(meeting_id, participant_id, "## Agenda\n\nBudget.");

    let note = world
        .queries()
        .note(meeting_id, participant_id)
        .unwrap()
        .expect("a note");
    assert_eq!(note.content, "## Agenda\n\nBudget.");
    assert_eq!(note.version, 1);
    assert_eq!(note.participant_id, participant_id);
    // A Host edit carries no author id, by constraint (ADR-0008).
    assert_eq!(note.last_author_type, "HOST");
    assert_eq!(note.last_author_id, None);
    assert_eq!(note.created_at, note.updated_at);
}

#[test]
fn the_note_reports_the_newest_version_and_who_wrote_it() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    world.write(meeting_id, participant_id, "first");
    world.write_as_participant(meeting_id, participant_id, "second");

    let note = world
        .queries()
        .note(meeting_id, participant_id)
        .unwrap()
        .expect("a note");
    assert_eq!(note.content, "second");
    assert_eq!(note.version, 2);
    // Authorship is the newest history row's, not the note's: a note has no
    // author column because three different actors can write it.
    assert_eq!(note.last_author_type, "PARTICIPANT");
    assert_eq!(note.last_author_id, Some(participant_id));
    assert!(note.updated_at >= note.created_at);
}

#[test]
fn a_note_is_scoped_to_its_own_meeting() {
    // The same participant id cannot exist in two meetings, but a wrong pair
    // must resolve to nothing rather than to somebody else's note.
    let world = World::new();
    let here = world.meeting("Weekly");
    let elsewhere = world.meeting("Budget Review");
    let mine = world.participant(here, "Budi Santoso");
    let theirs = world.participant(elsewhere, "Siti Rahayu");

    world.write(here, mine, "mine");
    world.write(elsewhere, theirs, "theirs");

    assert_eq!(world.queries().note(here, theirs).unwrap(), None);
    assert_eq!(world.queries().note(elsewhere, mine).unwrap(), None);
    assert_eq!(
        world.queries().note(here, mine).unwrap().unwrap().content,
        "mine"
    );
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[test]
fn history_is_newest_first_and_dense_from_one() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    for round in 1..=4 {
        world.write(meeting_id, participant_id, &format!("round {round}"));
    }

    let versions = world
        .queries()
        .note_versions(meeting_id, participant_id)
        .unwrap();
    assert_eq!(
        versions.iter().map(|v| v.version).collect::<Vec<_>>(),
        vec![4, 3, 2, 1]
    );
}

#[test]
fn a_history_row_carries_metadata_and_no_body() {
    // The list is metadata. Shipping every historical body to render a list of
    // dates would move a note's whole past across the boundary each time the
    // tab opened.
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    world.write(meeting_id, participant_id, "héllo");

    let versions = world
        .queries()
        .note_versions(meeting_id, participant_id)
        .unwrap();
    let first = versions.first().expect("a version");
    assert_eq!(first.version, 1);
    assert_eq!(first.created_by_type, "HOST");
    assert_eq!(first.created_by, None);
    // Bytes, not characters: "héllo" is six bytes and five characters, and the
    // domain's size limit is expressed in bytes.
    assert_eq!(first.byte_length, 6);
}

#[test]
fn history_records_who_wrote_each_version() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    world.write_as_participant(meeting_id, participant_id, "theirs");
    world.write(meeting_id, participant_id, "the host's");

    let versions = world
        .queries()
        .note_versions(meeting_id, participant_id)
        .unwrap();
    assert_eq!(versions[0].created_by_type, "HOST");
    assert_eq!(versions[0].created_by, None);
    assert_eq!(versions[1].created_by_type, "PARTICIPANT");
    assert_eq!(versions[1].created_by, Some(participant_id));
}

#[test]
fn history_is_empty_for_a_participant_with_no_note() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    assert!(world
        .queries()
        .note_versions(meeting_id, participant_id)
        .unwrap()
        .is_empty());
}

#[test]
fn one_version_can_be_fetched_with_its_body() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    world.write(meeting_id, participant_id, "first");
    world.write(meeting_id, participant_id, "second");

    let earlier = world
        .queries()
        .note_version(meeting_id, participant_id, 1)
        .unwrap()
        .expect("version 1");
    assert_eq!(earlier.content, "first");
    assert_eq!(earlier.version, 1);
    assert_eq!(earlier.created_by_type, "HOST");

    // History is never rewritten, so the old body is still the old body.
    assert_eq!(
        world
            .queries()
            .note_version(meeting_id, participant_id, 2)
            .unwrap()
            .expect("version 2")
            .content,
        "second"
    );
}

#[test]
fn a_version_that_does_not_exist_reads_as_absent() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.write(meeting_id, participant_id, "only one");

    for version in [0, 2, -1, i64::MAX] {
        assert_eq!(
            world
                .queries()
                .note_version(meeting_id, participant_id, version)
                .unwrap(),
            None,
            "version {version}"
        );
    }
}

#[test]
fn a_version_is_scoped_to_its_own_meeting() {
    let world = World::new();
    let here = world.meeting("Weekly");
    let elsewhere = world.meeting("Budget Review");
    let mine = world.participant(here, "Budi Santoso");
    let theirs = world.participant(elsewhere, "Siti Rahayu");
    world.write(elsewhere, theirs, "theirs");

    assert_eq!(world.queries().note_version(here, theirs, 1).unwrap(), None);
    assert!(world
        .queries()
        .note_versions(here, theirs)
        .unwrap()
        .is_empty());
    assert_eq!(world.queries().note_version(here, mine, 1).unwrap(), None);
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

#[test]
fn the_overview_lists_every_participant_including_those_without_a_note() {
    // The Host's notes list is the roster. Showing only the people who have
    // already written would hide exactly the ones being looked for.
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let alice = world.participant(meeting_id, "Alice Anwar");
    let bob = world.participant(meeting_id, "Bob Basuki");
    world.participant(meeting_id, "Citra Dewi");

    world.write(meeting_id, alice, "written");

    let overview = world.queries().notes_overview(meeting_id).unwrap();
    assert_eq!(overview.len(), 3);

    let first = &overview[0];
    assert_eq!(first.participant_id, alice);
    assert_eq!(first.name, "Alice Anwar");
    assert!(first.note_id.is_some());
    assert_eq!(first.version, Some(1));
    assert!(first.updated_at.is_some());

    let second = &overview[1];
    assert_eq!(second.participant_id, bob);
    assert_eq!(second.note_id, None);
    assert_eq!(second.version, None);
    assert_eq!(second.updated_at, None);
}

#[test]
fn the_overview_reports_the_current_version_per_participant() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let alice = world.participant(meeting_id, "Alice Anwar");
    let bob = world.participant(meeting_id, "Bob Basuki");

    world.write(meeting_id, alice, "one");
    world.write(meeting_id, alice, "two");
    world.write(meeting_id, alice, "three");
    world.write(meeting_id, bob, "just one");

    let overview = world.queries().notes_overview(meeting_id).unwrap();
    assert_eq!(overview[0].version, Some(3));
    assert_eq!(overview[1].version, Some(1));
}

#[test]
fn the_overview_orders_the_same_way_the_roster_does() {
    // Two lists of the same people in two orders is a UI that looks broken.
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    for name in ["Citra Dewi", "Alice Anwar", "Bob Basuki", "Alice Anwar"] {
        world.participant(meeting_id, name);
    }

    let overview: Vec<ParticipantId> = world
        .queries()
        .notes_overview(meeting_id)
        .unwrap()
        .into_iter()
        .map(|row| row.participant_id)
        .collect();
    let roster: Vec<ParticipantId> = world
        .queries()
        .participants(meeting_id)
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .collect();

    assert_eq!(overview, roster);
    assert_eq!(overview.len(), 4);
}

#[test]
fn the_overview_is_scoped_to_one_meeting() {
    let world = World::new();
    let here = world.meeting("Weekly");
    let elsewhere = world.meeting("Budget Review");
    world.participant(here, "Budi Santoso");
    let theirs = world.participant(elsewhere, "Siti Rahayu");
    world.write(elsewhere, theirs, "theirs");

    let overview = world.queries().notes_overview(here).unwrap();
    assert_eq!(overview.len(), 1);
    assert_eq!(overview[0].note_id, None);
}

#[test]
fn a_meeting_with_no_roster_has_an_empty_overview() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    assert!(world
        .queries()
        .notes_overview(meeting_id)
        .unwrap()
        .is_empty());
}

// ---------------------------------------------------------------------------
// The participant's own read model
// ---------------------------------------------------------------------------

#[test]
fn a_participant_with_no_note_reads_as_absent() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    assert_eq!(
        ParticipantQueries::new(&world.db)
            .own_note(meeting_id, participant_id)
            .unwrap(),
        None
    );
}

#[test]
fn a_participants_own_note_reads_back_with_its_version_and_author() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.write_as_participant(meeting_id, participant_id, "mine");

    let note = ParticipantQueries::new(&world.db)
        .own_note(meeting_id, participant_id)
        .unwrap()
        .expect("a note");
    assert_eq!(note.content, "mine");
    assert_eq!(note.version, 1);
    assert_eq!(note.last_author_type, "PARTICIPANT");
}

#[test]
fn a_participant_can_see_that_the_host_changed_their_note() {
    // The one piece of provenance the participant shape carries, and the
    // reason it carries it (ADR-0020).
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    world.write_as_participant(meeting_id, participant_id, "mine");
    world.write(meeting_id, participant_id, "the host's correction");

    let note = ParticipantQueries::new(&world.db)
        .own_note(meeting_id, participant_id)
        .unwrap()
        .expect("a note");
    assert_eq!(note.content, "the host's correction");
    assert_eq!(note.version, 2);
    assert_eq!(note.last_author_type, "HOST");
}

#[test]
fn the_participant_read_model_cannot_reach_another_participants_note() {
    // Scoped by both ids, and there is no route that passes a participant id
    // here (PRD section 5.2).
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let alice = world.participant(meeting_id, "Alice Anwar");
    let bob = world.participant(meeting_id, "Bob Basuki");

    world.write_as_participant(meeting_id, alice, "alice's");
    world.write_as_participant(meeting_id, bob, "bob's");

    let queries = ParticipantQueries::new(&world.db);
    assert_eq!(
        queries
            .own_note(meeting_id, alice)
            .unwrap()
            .unwrap()
            .content,
        "alice's"
    );
    assert_eq!(
        queries.own_note(meeting_id, bob).unwrap().unwrap().content,
        "bob's"
    );
}

#[test]
fn the_participant_read_model_is_scoped_to_its_own_meeting() {
    let world = World::new();
    let here = world.meeting("Weekly");
    let elsewhere = world.meeting("Budget Review");
    let mine = world.participant(here, "Budi Santoso");
    let theirs = world.participant(elsewhere, "Siti Rahayu");
    world.write_as_participant(elsewhere, theirs, "theirs");

    let queries = ParticipantQueries::new(&world.db);
    assert_eq!(queries.own_note(here, theirs).unwrap(), None);
    assert_eq!(queries.own_note(elsewhere, mine).unwrap(), None);
}

#[test]
fn the_participant_read_model_survives_a_lock() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.write_as_participant(meeting_id, participant_id, "kept");

    world
        .domain
        .open_meeting(&Actor::Host, meeting_id)
        .expect("open");
    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");

    assert!(ParticipantQueries::new(&world.db)
        .own_note(meeting_id, participant_id)
        .unwrap()
        .is_some());
}

// ---------------------------------------------------------------------------
// A participant writing their own note, through the domain
// ---------------------------------------------------------------------------

#[test]
fn a_participant_write_is_attributed_to_the_participant() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.write_as_participant(meeting_id, participant_id, "mine");

    let versions = world
        .queries()
        .note_versions(meeting_id, participant_id)
        .unwrap();
    assert_eq!(versions[0].created_by_type, "PARTICIPANT");
    assert_eq!(versions[0].created_by, Some(participant_id));
}

#[test]
fn a_participant_cannot_write_another_participants_note() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let alice = world.participant(meeting_id, "Alice Anwar");
    let bob = world.participant(meeting_id, "Bob Basuki");

    let refused = world.domain.write_note(
        &Actor::Participant {
            meeting_id,
            participant_id: alice,
            session_id: app_core::id::SessionId::new(),
        },
        WriteNote {
            meeting_id,
            participant_id: bob,
            content: "not mine to write".to_owned(),
        },
    );

    assert!(refused.is_err());
    assert_eq!(
        ParticipantQueries::new(&world.db)
            .own_note(meeting_id, bob)
            .unwrap(),
        None
    );
}

#[test]
fn a_participant_cannot_act_in_another_meeting() {
    let world = World::new();
    let here = world.meeting("Weekly");
    let elsewhere = world.meeting("Budget Review");
    let mine = world.participant(here, "Budi Santoso");
    let theirs = world.participant(elsewhere, "Siti Rahayu");

    let refused = world.domain.write_note(
        &Actor::Participant {
            meeting_id: here,
            participant_id: mine,
            session_id: app_core::id::SessionId::new(),
        },
        WriteNote {
            meeting_id: elsewhere,
            participant_id: theirs,
            content: "reaching across".to_owned(),
        },
    );

    assert!(refused.is_err());
}

#[test]
fn a_locked_meeting_refuses_a_participant_write() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.write_as_participant(meeting_id, participant_id, "before");

    world
        .domain
        .open_meeting(&Actor::Host, meeting_id)
        .expect("open");
    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");

    let refused = world.domain.write_note(
        &Actor::Participant {
            meeting_id,
            participant_id,
            session_id: app_core::id::SessionId::new(),
        },
        WriteNote {
            meeting_id,
            participant_id,
            content: "after".to_owned(),
        },
    );
    assert!(refused.is_err());

    assert_eq!(
        ParticipantQueries::new(&world.db)
            .own_note(meeting_id, participant_id)
            .unwrap()
            .unwrap()
            .content,
        "before"
    );
}

#[test]
fn a_refused_participant_write_creates_neither_a_version_nor_an_audit_entry() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    let before = world.queries().audit_entries(meeting_id).unwrap().len();

    let refused = world.domain.write_note(
        &Actor::Participant {
            meeting_id,
            participant_id,
            session_id: app_core::id::SessionId::new(),
        },
        WriteNote {
            meeting_id,
            participant_id,
            content: "<script>alert(1)</script>".to_owned(),
        },
    );
    assert!(refused.is_err());

    assert!(world
        .queries()
        .note_versions(meeting_id, participant_id)
        .unwrap()
        .is_empty());
    assert_eq!(
        world.queries().audit_entries(meeting_id).unwrap().len(),
        before
    );
}

#[test]
fn a_participant_write_is_audited_as_the_participant_without_the_body() {
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");

    let secret = "Budget overrun of 40 percent";
    world.write_as_participant(meeting_id, participant_id, secret);

    let entries = world.queries().audit_entries(meeting_id).unwrap();
    let note_entry = entries
        .iter()
        .find(|entry| entry.action.starts_with("note."))
        .expect("a note audit entry");

    assert_eq!(note_entry.action, "note.created");
    assert_eq!(note_entry.actor_type, "PARTICIPANT");
    assert_eq!(note_entry.actor_id, Some(participant_id));
    assert_eq!(note_entry.target_type, "note");

    let metadata = note_entry.metadata.as_ref().expect("metadata").to_string();
    assert!(metadata.contains("\"version\":1"), "{metadata}");
    assert!(!metadata.contains("Budget overrun"), "{metadata}");
}

// ---------------------------------------------------------------------------
// The read path does not decide anything
// ---------------------------------------------------------------------------

#[test]
fn locking_a_meeting_does_not_hide_its_notes() {
    // Reads are unaffected by the lock: a locked meeting is finished, not
    // secret, and its notes are exactly what the Host locked it to keep.
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.write(meeting_id, participant_id, "kept");

    world
        .domain
        .open_meeting(&Actor::Host, meeting_id)
        .expect("open");
    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");

    assert!(world
        .queries()
        .note(meeting_id, participant_id)
        .unwrap()
        .is_some());
    assert_eq!(
        world
            .queries()
            .note_versions(meeting_id, participant_id)
            .unwrap()
            .len(),
        1
    );
    assert!(world
        .queries()
        .note_version(meeting_id, participant_id, 1)
        .unwrap()
        .is_some());

    // And a write is still refused, which is where the lock lives.
    assert!(world
        .domain
        .write_note(
            &Actor::Host,
            WriteNote {
                meeting_id,
                participant_id,
                content: "too late".to_owned(),
            },
        )
        .is_err());
}

#[test]
fn removing_a_participant_takes_their_note_and_history_with_them() {
    // `ON DELETE CASCADE` on the composite foreign key. Removal is DRAFT-only,
    // so this is the Host correcting a roster before the meeting opens rather
    // than history being erased afterwards (ADR-0013).
    let world = World::new();
    let meeting_id = world.meeting("Weekly");
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.write(meeting_id, participant_id, "draft thoughts");

    world
        .domain
        .remove_participant(&Actor::Host, meeting_id, participant_id)
        .expect("remove");

    assert_eq!(
        world.queries().note(meeting_id, participant_id).unwrap(),
        None
    );
    assert!(world
        .queries()
        .note_versions(meeting_id, participant_id)
        .unwrap()
        .is_empty());
    assert!(world
        .queries()
        .notes_overview(meeting_id)
        .unwrap()
        .is_empty());
}
