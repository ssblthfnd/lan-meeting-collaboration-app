//! Event publication and presence, against a real SQLite database.
//!
//! The questions here are all about **ordering and independence**:
//!
//! - does an event only ever describe something that committed;
//! - does a refusal stay silent;
//! - can a sink that fails take a durable write down with it.
//!
//! They live in `app-db` for the same reason the domain rule tests do: the
//! claim being tested is "the row is there and the event followed it", which
//! needs a real transaction to mean anything. `app-core` keeps the tests about
//! the audience table, which need no database at all.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use app_core::actor::Actor;
use app_core::event::{DomainEvent, EventSink};
use app_core::id::{MeetingId, ParticipantId, SessionId};
use app_core::meeting::{MeetingConfiguration, MeetingStatus};
use app_core::participant::ParticipantDetails;
use app_core::service::{Domain, WriteNote};
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone, UtcTimestamp};
use app_core::token::TokenHash;
use app_db::presence::PresenceStore;
use app_db::Db;
use tempfile::TempDir;

/// A sink that keeps what it was handed, in order.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<DomainEvent>>,
}

impl Recorder {
    fn kinds(&self) -> Vec<&'static str> {
        self.seen
            .lock()
            .expect("not poisoned")
            .iter()
            .map(DomainEvent::kind)
            .collect()
    }

    fn last(&self) -> Option<DomainEvent> {
        self.seen.lock().expect("not poisoned").last().cloned()
    }

    fn clear(&self) {
        self.seen.lock().expect("not poisoned").clear();
    }

    fn count(&self) -> usize {
        self.seen.lock().expect("not poisoned").len()
    }
}

impl EventSink for Recorder {
    fn publish(&self, event: &DomainEvent) {
        self.seen.lock().expect("not poisoned").push(event.clone());
    }
}

/// A sink whose delivery never succeeds.
///
/// The shape of every real one: a broadcast channel with no receivers, a window
/// that has closed. It counts attempts so a test can prove the domain tried and
/// carried on regardless.
#[derive(Default)]
struct DeliveryAlwaysFails {
    attempts: AtomicUsize,
}

impl EventSink for DeliveryAlwaysFails {
    fn publish(&self, _event: &DomainEvent) {
        // A real sink would discard a `SendError` here. There is nothing to
        // return: `publish` cannot fail, which is what makes a committed
        // transaction independent of delivery.
        self.attempts.fetch_add(1, Ordering::SeqCst);
    }
}

struct World {
    db: Arc<Db>,
    domain: Domain<Arc<Db>>,
    events: Arc<Recorder>,
    _dir: TempDir,
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

    fn configuration() -> MeetingConfiguration {
        MeetingConfiguration {
            title: "Weekly Coordination".to_owned(),
            topic: Some("Budget".to_owned()),
            date: MeetingDate::new(2026, 9, 20).unwrap(),
            start_time: MeetingTime::new(9, 0, 0).unwrap(),
            end_time: MeetingTime::new(10, 30, 0).unwrap(),
            timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
            location: None,
            description: None,
        }
    }

    fn meeting(&self) -> MeetingId {
        self.domain
            .create_meeting(&Actor::Host, Self::configuration())
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

    /// An open meeting with one claimed identity.
    fn joined(&self) -> (MeetingId, ParticipantId, SessionId) {
        let meeting_id = self.meeting();
        let participant_id = self.participant(meeting_id, "Budi Santoso");
        self.domain
            .open_meeting(&Actor::Host, meeting_id)
            .expect("open");

        let hash = TokenHash::parse(&"a".repeat(64)).expect("hash");
        let claimed = self
            .domain
            .claim_identity(
                &Actor::Claimant {
                    meeting_id,
                    participant_id,
                },
                meeting_id,
                participant_id,
                &hash,
            )
            .expect("claim");

        (meeting_id, participant_id, claimed.session_id)
    }
}

// ---------------------------------------------------------------------------
// Ordering: an event describes something that already committed
// ---------------------------------------------------------------------------

#[test]
fn a_successful_mutation_commits_before_its_event_is_published() {
    // The ordering architecture rules section 16 requires, asserted from the
    // sink itself: by the time `publish` runs, the row must already be
    // readable on a *different* connection, which only a committed transaction
    // allows. A pre-commit publish would find nothing - the writer's
    // uncommitted row is invisible to the read pool.
    struct AssertsCommitted {
        db: Arc<Db>,
        checked: AtomicUsize,
    }

    impl EventSink for AssertsCommitted {
        fn publish(&self, event: &DomainEvent) {
            if let DomainEvent::ParticipantAdded { participant_id, .. } = event {
                let found = app_db::query::HostQueries::new(&self.db)
                    .participants(event.meeting_id())
                    .expect("read")
                    .into_iter()
                    .any(|row| row.id == *participant_id);
                assert!(found, "the event outran the commit: {event:?}");
                self.checked.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open"));
    let sink = Arc::new(AssertsCommitted {
        db: Arc::clone(&db),
        checked: AtomicUsize::new(0),
    });
    let domain = Domain::with_events(Arc::clone(&db), Arc::clone(&sink) as Arc<dyn EventSink>);

    let meeting_id = domain
        .create_meeting(&Actor::Host, World::configuration())
        .expect("create")
        .meeting_id;
    domain
        .add_participant(
            &Actor::Host,
            meeting_id,
            ParticipantDetails {
                name: "Budi Santoso".to_owned(),
                department: None,
                position: None,
                meeting_role: None,
            },
        )
        .expect("add");

    assert_eq!(sink.checked.load(Ordering::SeqCst), 1);
}

#[test]
fn a_rolled_back_mutation_publishes_nothing() {
    // The lock refusal is read inside the transaction, so the write and its
    // audit row are both rolled back. Nothing may be announced about a state
    // the database never reached.
    let world = World::new();
    let meeting_id = world.meeting();
    world
        .domain
        .open_meeting(&Actor::Host, meeting_id)
        .expect("open");
    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");
    world.events.clear();

    let refused = world.domain.add_participant(
        &Actor::Host,
        meeting_id,
        ParticipantDetails {
            name: "Too Late".to_owned(),
            department: None,
            position: None,
            meeting_role: None,
        },
    );

    assert!(refused.is_err());
    assert_eq!(world.events.kinds(), Vec::<&str>::new());
}

#[test]
fn a_validation_failure_publishes_nothing() {
    let world = World::new();
    let meeting_id = world.meeting();
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.events.clear();

    let refused = world.domain.write_note(
        &Actor::Host,
        WriteNote {
            meeting_id,
            participant_id,
            content: "   ".to_owned(),
        },
    );

    assert!(refused.is_err());
    assert_eq!(world.events.count(), 0);
}

#[test]
fn an_authorization_failure_publishes_nothing() {
    // A claimant holding a join token may claim, and nothing else. The refusal
    // happens before any write, and it must also happen before any event.
    let world = World::new();
    let meeting_id = world.meeting();
    let mine = world.participant(meeting_id, "Budi Santoso");
    let theirs = world.participant(meeting_id, "Siti Rahayu");
    world.events.clear();

    let refused = world.domain.write_note(
        &Actor::Claimant {
            meeting_id,
            participant_id: mine,
        },
        WriteNote {
            meeting_id,
            participant_id: theirs,
            content: "Not mine to write.".to_owned(),
        },
    );

    assert!(refused.is_err());
    assert_eq!(world.events.count(), 0);
}

#[test]
fn a_sink_whose_delivery_fails_does_not_roll_back_a_committed_write() {
    // The one-way dependency. SQLite must not care whether anybody heard.
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open"));
    let sink = Arc::new(DeliveryAlwaysFails::default());
    let domain = Domain::with_events(Arc::clone(&db), Arc::clone(&sink) as Arc<dyn EventSink>);

    let created = domain
        .create_meeting(&Actor::Host, World::configuration())
        .expect("a failed delivery must not fail the mutation");

    assert!(
        sink.attempts.load(Ordering::SeqCst) >= 1,
        "nothing was tried"
    );

    // The committed fact survives, read back on another connection.
    let stored = app_db::query::HostQueries::new(&db)
        .meeting(created.meeting_id)
        .expect("read")
        .expect("the meeting is there");
    assert_eq!(stored.status, MeetingStatus::Draft);
}

// ---------------------------------------------------------------------------
// The events each mutation produces
// ---------------------------------------------------------------------------

#[test]
fn each_mutation_publishes_exactly_one_event_naming_what_it_changed() {
    let world = World::new();

    let meeting_id = world.meeting();
    assert_eq!(world.events.kinds(), vec!["meeting.created"]);
    world.events.clear();

    world
        .domain
        .update_meeting(&Actor::Host, meeting_id, World::configuration())
        .expect("update");
    assert_eq!(world.events.kinds(), vec!["meeting.updated"]);
    world.events.clear();

    let participant_id = world.participant(meeting_id, "Budi Santoso");
    assert_eq!(world.events.kinds(), vec!["participant.added"]);
    assert_eq!(
        world.events.last().and_then(|e| e.participant_id()),
        Some(participant_id)
    );
    world.events.clear();

    world
        .domain
        .open_meeting(&Actor::Host, meeting_id)
        .expect("open");
    assert_eq!(world.events.kinds(), vec!["meeting.opened"]);
    world.events.clear();

    let hash = TokenHash::parse(&"b".repeat(64)).expect("hash");
    world
        .domain
        .issue_join_token(&Actor::Host, meeting_id, &hash)
        .expect("issue");
    assert_eq!(world.events.kinds(), vec!["meeting.join_token_issued"]);
    world.events.clear();

    world
        .domain
        .lock_meeting(&Actor::Host, meeting_id)
        .expect("lock");
    assert_eq!(world.events.kinds(), vec!["meeting.locked"]);
}

#[test]
fn a_claim_and_the_host_actions_on_it_publish_the_session_events() {
    let world = World::new();
    let (meeting_id, participant_id, session_id) = world.joined();

    assert_eq!(
        world.events.last().map(|e| e.kind()),
        Some("participant.claimed")
    );
    // A claim announces the person, not the session: only revocation needs to
    // name a socket.
    assert_eq!(world.events.last().and_then(|e| e.session_id()), None);
    world.events.clear();

    world
        .domain
        .approve_claim(&Actor::Host, meeting_id, participant_id)
        .expect("approve");
    assert_eq!(world.events.kinds(), vec!["claim.approved"]);
    world.events.clear();

    world
        .domain
        .revoke_session(&Actor::Host, meeting_id, participant_id)
        .expect("revoke");
    let revoked = world.events.last().expect("an event");
    assert_eq!(revoked.kind(), "session.revoked");
    // The id comes from the row the transaction acted on, which is the id the
    // transport will match a socket against.
    assert_eq!(revoked.session_id(), Some(session_id));
    assert_eq!(revoked.participant_id(), Some(participant_id));
}

#[test]
fn a_note_event_carries_the_version_and_never_the_markdown() {
    let world = World::new();
    let meeting_id = world.meeting();
    let participant_id = world.participant(meeting_id, "Budi Santoso");
    world.events.clear();

    let secret = "Budget overrun of 40 percent, do not circulate.";
    world
        .domain
        .write_note(
            &Actor::Host,
            WriteNote {
                meeting_id,
                participant_id,
                content: secret.to_owned(),
            },
        )
        .expect("write");

    let event = world.events.last().expect("an event");
    assert_eq!(event.kind(), "note.changed");
    match event {
        DomainEvent::NoteChanged { version, .. } => assert_eq!(version, 1),
        other => panic!("{other:?}"),
    }
    assert!(!format!("{event:?}").contains("Budget overrun"));
}

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

#[test]
fn touching_a_session_records_when_it_was_last_seen() {
    let world = World::new();
    let (meeting_id, participant_id, session_id) = world.joined();
    let presence = PresenceStore::new(&world.db);

    let before = presence.roster_presence(meeting_id).expect("read");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].participant_id, participant_id);
    assert_eq!(before[0].session_id, Some(session_id));
    // Claimed over HTTP, never connected.
    assert_eq!(before[0].last_seen_at, None);

    let at = UtcTimestamp::now();
    assert!(presence.touch_session(session_id, at).expect("touch"));

    let after = presence.roster_presence(meeting_id).expect("read");
    assert_eq!(after[0].last_seen_at, Some(at));
}

#[test]
fn a_revoked_session_can_no_longer_be_touched() {
    // `revoked_at IS NULL` is in the statement, not in a caller's memory. A
    // socket that is closing after its session was revoked must not be able to
    // write a fresher timestamp onto a dead row.
    let world = World::new();
    let (meeting_id, participant_id, session_id) = world.joined();
    let presence = PresenceStore::new(&world.db);

    presence
        .touch_session(session_id, UtcTimestamp::now())
        .expect("touch");
    world
        .domain
        .revoke_session(&Actor::Host, meeting_id, participant_id)
        .expect("revoke");

    assert!(
        !presence
            .touch_session(session_id, UtcTimestamp::now())
            .expect("touch"),
        "a revoked session must not be touchable"
    );

    // And the identity now reads as held by nobody.
    let roster = presence.roster_presence(meeting_id).expect("read");
    assert_eq!(roster[0].session_id, None);
    assert_eq!(roster[0].last_seen_at, None);
}

#[test]
fn touching_a_session_that_does_not_exist_changes_nothing() {
    let world = World::new();
    let (meeting_id, _, _) = world.joined();
    let presence = PresenceStore::new(&world.db);

    assert!(!presence
        .touch_session(SessionId::new(), UtcTimestamp::now())
        .expect("touch"));
    assert_eq!(
        presence.roster_presence(meeting_id).expect("read")[0].last_seen_at,
        None
    );
}

#[test]
fn the_roster_lists_every_identity_including_the_unclaimed_ones() {
    // A presence view with holes in it would make the Host's roster and their
    // participant list disagree about how many people there are.
    let world = World::new();
    let meeting_id = world.meeting();
    world.participant(meeting_id, "Siti Rahayu");
    world.participant(meeting_id, "Budi Santoso");
    world.participant(meeting_id, "Agus Wijaya");

    let roster = PresenceStore::new(&world.db)
        .roster_presence(meeting_id)
        .expect("read");

    assert_eq!(roster.len(), 3);
    assert!(roster.iter().all(|row| row.session_id.is_none()));

    // Ordered by name, then id: the same total ordering the roster query uses.
    let names: Vec<ParticipantId> = roster.iter().map(|row| row.participant_id).collect();
    let expected: Vec<ParticipantId> = app_db::query::HostQueries::new(&world.db)
        .participants(meeting_id)
        .expect("read")
        .into_iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(names, expected);
}

#[test]
fn presence_never_reaches_the_audit_log() {
    // Architecture rules section 17: the audit log records what people did to
    // the meeting. A socket opening is not one of those, and burying the real
    // entries under connection noise would make the log useless (ADR-0018).
    let world = World::new();
    let (meeting_id, _, session_id) = world.joined();
    let presence = PresenceStore::new(&world.db);

    let before = app_db::query::HostQueries::new(&world.db)
        .audit_entries(meeting_id)
        .expect("read")
        .len();

    for _ in 0..5 {
        presence
            .touch_session(session_id, UtcTimestamp::now())
            .expect("touch");
    }

    let after = app_db::query::HostQueries::new(&world.db)
        .audit_entries(meeting_id)
        .expect("read");
    assert_eq!(after.len(), before);
    assert!(after.iter().all(|entry| !entry.action.contains("presence")));
}
