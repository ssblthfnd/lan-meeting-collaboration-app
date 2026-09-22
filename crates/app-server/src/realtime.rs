//! The broadcast channel and the connection registry.
//!
//! Two small pieces of shared state, both deliberately in memory only:
//!
//! - a bounded [`tokio::sync::broadcast`] channel carrying committed
//!   [`DomainEvent`]s to every open socket;
//! - a count of open sockets per participant identity, which is what presence
//!   is derived from.
//!
//! # Why the channel is bounded
//!
//! A participant whose phone went into a tunnel stops reading. With an
//! unbounded queue that participant's backlog would grow until the Host's
//! machine noticed, and a mutation would be arranging memory on behalf of
//! somebody who has gone away. Bounded means the opposite failure: the slow
//! reader is told it fell behind and is closed, and it reconnects and re-reads
//! the current state over HTTP.
//!
//! That is the correct trade because the channel is **not** the source of
//! truth. An event is a hint to go and look; SQLite holds the answer
//! (architecture rules section 16). Dropping a hint costs a refetch, and a
//! refetch is what the client does on reconnect anyway - which is why there is
//! no replay log, no acknowledgement and no per-client queue here (ADR-0018).
//!
//! [`Realtime::publish`] never blocks and never fails. A committed transaction
//! cannot be undone by a client that stopped listening, so there is nothing for
//! it to report.
//!
//! # Why presence is not a column
//!
//! A count of open sockets is true only while this process is running. If the
//! Host's machine loses power, a `connected` column in SQLite would still say
//! everyone is here, forever, and every reader would have to know not to
//! believe it. Keeping it in memory means it is empty after a restart, which is
//! the truth: nobody is connected to a server that just started.
//!
//! The durable half - when a socket was last observed - lives in
//! `participant_sessions.last_seen_at` and is written by `app_db::presence`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use app_core::event::{DomainEvent, EventSink};
use app_core::id::{MeetingId, ParticipantId};
use tokio::sync::broadcast;

/// Events held for a socket that is not reading.
///
/// Generous for a meeting: 99 participants cannot between them produce this
/// many events in the time a healthy socket takes to drain one. A client that
/// falls this far behind is not slow, it is gone - and telling it to resync is
/// both cheaper and more correct than growing a queue for it.
pub const EVENT_QUEUE: usize = 256;

/// The shared realtime state of one Host process.
///
/// Held by the Tauri shell and handed to the LAN server, so that it survives
/// the server being stopped and started: the channel stays valid, and the
/// registry empties by itself as the sockets close.
pub struct Realtime {
    events: broadcast::Sender<Arc<DomainEvent>>,
    /// Open sockets per identity. An entry exists only while the count is
    /// above zero, so the map is empty when nobody is connected.
    sockets: Mutex<HashMap<(MeetingId, ParticipantId), usize>>,
    /// Sockets subscribed since this process started. Only ever rises.
    served: AtomicU64,
}

impl Default for Realtime {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Realtime {
    /// Reports sizes, never contents.
    ///
    /// A registry keyed by identity is not a credential, but it is a list of
    /// who is in the room, and a diagnostic has no reason to print one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Realtime")
            .field("subscribers", &self.subscriber_count())
            .field("open_sockets", &self.open_socket_count())
            .field("served", &self.sockets_served())
            .finish()
    }
}

impl Realtime {
    /// A fresh channel with no subscribers and nobody connected.
    #[must_use]
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_QUEUE);
        Self {
            events,
            sockets: Mutex::new(HashMap::new()),
            served: AtomicU64::new(0),
        }
    }

    /// The registry, recovering a poisoned lock rather than discarding it.
    ///
    /// The same reasoning as the LAN lifecycle mutex in the Tauri shell:
    /// nothing inside the critical sections can panic, and if one somehow did,
    /// the protected map is still a whole, valid map. Treating presence as
    /// unknowable from then on would be worse than reading a map that is
    /// correct.
    fn sockets(&self) -> MutexGuard<'_, HashMap<(MeetingId, ParticipantId), usize>> {
        self.sockets.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A receiver for every event published from now on.
    ///
    /// No history: a socket that has just opened is about to re-read current
    /// state over HTTP anyway, so replaying what it missed would tell it things
    /// it is already about to learn (ADR-0018).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<DomainEvent>> {
        self.served.fetch_add(1, Ordering::SeqCst);
        self.events.subscribe()
    }

    /// Record one more open socket for an identity.
    ///
    /// Returns `true` on the `0 -> 1` transition - the moment the participant
    /// became present. A second tab or a phone returns `false`, which is what
    /// keeps a roster from flickering when somebody opens the meeting twice.
    pub fn attach(&self, meeting_id: MeetingId, participant_id: ParticipantId) -> bool {
        let mut sockets = self.sockets();
        let count = sockets.entry((meeting_id, participant_id)).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// Record one fewer open socket for an identity.
    ///
    /// Returns `true` on the `1 -> 0` transition - the moment the participant
    /// stopped being present. The entry is removed at zero so the map holds
    /// only who is actually here.
    pub fn detach(&self, meeting_id: MeetingId, participant_id: ParticipantId) -> bool {
        let mut sockets = self.sockets();
        let key = (meeting_id, participant_id);
        match sockets.get_mut(&key) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                sockets.remove(&key);
                true
            }
            // Detaching something that was never attached. Not reachable from
            // the socket handler, which attaches exactly once, and reporting a
            // transition here would emit a disconnect nobody connected for.
            None => false,
        }
    }

    /// Whether any socket is open for this identity.
    #[must_use]
    pub fn is_connected(&self, meeting_id: MeetingId, participant_id: ParticipantId) -> bool {
        self.sockets().contains_key(&(meeting_id, participant_id))
    }

    /// Everyone currently connected to one meeting.
    ///
    /// Sorted, so a caller rendering it has a total order without inventing one
    /// (architecture rules section 26.4). Hash map iteration order is not one.
    #[must_use]
    pub fn connected(&self, meeting_id: MeetingId) -> Vec<ParticipantId> {
        let mut found: Vec<ParticipantId> = self
            .sockets()
            .keys()
            .filter(|(meeting, _)| *meeting == meeting_id)
            .map(|(_, participant)| *participant)
            .collect();
        found.sort_unstable();
        found
    }

    /// How many sockets are open in total, across every meeting.
    #[must_use]
    pub fn open_socket_count(&self) -> usize {
        self.sockets().values().sum()
    }

    /// How many receivers are subscribed to the channel right now.
    ///
    /// One per open socket.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.events.receiver_count()
    }

    /// How many sockets have subscribed since this process started.
    ///
    /// Only ever rises, which is what makes it usable as a happens-before
    /// marker. A socket subscribes before it attaches and before it reads
    /// anything, so once this has passed the value taken before a handshake, an
    /// event published now is certain to reach that socket.
    /// [`Realtime::subscriber_count`] cannot answer that question, because a
    /// socket closing elsewhere moves it the other way at the same time.
    #[must_use]
    pub fn sockets_served(&self) -> u64 {
        self.served.load(Ordering::SeqCst)
    }
}

impl EventSink for Realtime {
    /// Offer the event to every open socket, and carry on regardless.
    ///
    /// `send` fails when there are no subscribers, which is the ordinary state
    /// of a Host who has not started the LAN server. It is discarded: the
    /// transaction that produced this event has already committed, and nothing
    /// here may suggest otherwise (architecture rules section 16).
    fn publish(&self, event: &DomainEvent) {
        let _ = self.events.send(Arc::new(event.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::time::UtcTimestamp;

    fn locked(meeting_id: MeetingId) -> DomainEvent {
        DomainEvent::MeetingLocked {
            meeting_id,
            at: UtcTimestamp::now(),
        }
    }

    #[test]
    fn publishing_with_nobody_listening_is_not_a_failure() {
        // The ordinary state of a Host who has not started the server. A
        // mutation must not care.
        let realtime = Realtime::new();
        realtime.publish(&locked(MeetingId::new()));
    }

    #[test]
    fn the_first_socket_is_a_transition_and_the_second_is_not() {
        let realtime = Realtime::new();
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();

        assert!(realtime.attach(meeting_id, participant_id), "0 -> 1");
        assert!(!realtime.attach(meeting_id, participant_id), "1 -> 2");
        assert!(!realtime.attach(meeting_id, participant_id), "2 -> 3");
        assert!(realtime.is_connected(meeting_id, participant_id));
        assert_eq!(realtime.open_socket_count(), 3);
    }

    #[test]
    fn only_the_last_socket_closing_is_a_transition() {
        // A laptop and a phone: closing one must not announce that the
        // participant left the meeting.
        let realtime = Realtime::new();
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();

        realtime.attach(meeting_id, participant_id);
        realtime.attach(meeting_id, participant_id);

        assert!(!realtime.detach(meeting_id, participant_id), "2 -> 1");
        assert!(realtime.is_connected(meeting_id, participant_id));

        assert!(realtime.detach(meeting_id, participant_id), "1 -> 0");
        assert!(!realtime.is_connected(meeting_id, participant_id));
        assert_eq!(realtime.open_socket_count(), 0);
    }

    #[test]
    fn a_subscriber_is_counted_from_the_moment_it_subscribes() {
        // What makes "this socket can now receive" observable. A broadcast
        // channel delivers to receivers that existed when an event was sent,
        // so a count that lagged would be a count nothing could be timed
        // against.
        let realtime = Realtime::new();
        assert_eq!(realtime.subscriber_count(), 0);

        let first = realtime.subscribe();
        assert_eq!(realtime.subscriber_count(), 1);
        let second = realtime.subscribe();
        assert_eq!(realtime.subscriber_count(), 2);

        drop(first);
        assert_eq!(realtime.subscriber_count(), 1);
        drop(second);
        assert_eq!(realtime.subscriber_count(), 0);
    }

    #[test]
    fn the_served_total_only_ever_rises() {
        // The live count moves both ways, so it cannot say "this new socket is
        // ready" while an older one happens to be closing. This one can.
        let realtime = Realtime::new();
        assert_eq!(realtime.sockets_served(), 0);

        let first = realtime.subscribe();
        assert_eq!(realtime.subscriber_count(), 1);
        assert_eq!(realtime.sockets_served(), 1);

        drop(first);
        assert_eq!(realtime.subscriber_count(), 0);
        assert_eq!(realtime.sockets_served(), 1, "a closed socket still served");

        let _second = realtime.subscribe();
        assert_eq!(realtime.sockets_served(), 2);
    }

    #[test]
    fn a_detach_without_an_attach_announces_nothing() {
        let realtime = Realtime::new();
        assert!(!realtime.detach(MeetingId::new(), ParticipantId::new()));
    }

    #[test]
    fn presence_is_scoped_to_one_meeting() {
        let realtime = Realtime::new();
        let here = MeetingId::new();
        let elsewhere = MeetingId::new();
        let alice = ParticipantId::new();
        let bob = ParticipantId::new();

        realtime.attach(here, alice);
        realtime.attach(elsewhere, bob);

        assert_eq!(realtime.connected(here), vec![alice]);
        assert_eq!(realtime.connected(elsewhere), vec![bob]);
        assert!(!realtime.is_connected(here, bob));
    }

    #[test]
    fn the_connected_list_has_a_total_order() {
        // Hash map iteration order is not one, and a roster rendered from this
        // must not shuffle between reads (architecture rules section 26.4).
        let realtime = Realtime::new();
        let meeting_id = MeetingId::new();
        let mut ids: Vec<ParticipantId> = (0..8).map(|_| ParticipantId::new()).collect();
        for id in &ids {
            realtime.attach(meeting_id, *id);
        }
        ids.sort_unstable();

        assert_eq!(realtime.connected(meeting_id), ids);
        assert_eq!(
            realtime.connected(meeting_id),
            realtime.connected(meeting_id)
        );
    }

    #[tokio::test]
    async fn a_subscriber_receives_what_is_published_after_it_subscribed() {
        let realtime = Realtime::new();
        let meeting_id = MeetingId::new();

        // Published before anybody subscribed: deliberately not replayed.
        realtime.publish(&locked(meeting_id));

        let mut first = realtime.subscribe();
        let mut second = realtime.subscribe();
        realtime.publish(&locked(meeting_id));

        for receiver in [&mut first, &mut second] {
            let event = receiver.recv().await.expect("an event");
            assert_eq!(event.kind(), "meeting.locked");
        }
        assert!(first.try_recv().is_err(), "exactly one event, not two");
    }

    #[tokio::test]
    async fn a_subscriber_that_stops_reading_is_told_it_fell_behind() {
        // The backpressure policy: the channel is bounded, so a socket that is
        // not draining is reported as lagged and will be closed and resynced,
        // rather than being allowed to grow a queue on the Host's machine.
        let realtime = Realtime::new();
        let meeting_id = MeetingId::new();
        let mut receiver = realtime.subscribe();

        for _ in 0..(EVENT_QUEUE + 8) {
            realtime.publish(&locked(meeting_id));
        }

        assert!(
            matches!(
                receiver.recv().await,
                Err(broadcast::error::RecvError::Lagged(_))
            ),
            "a bounded channel must report the overflow rather than hide it"
        );
    }

    #[tokio::test]
    async fn a_dropped_subscriber_does_not_affect_the_others() {
        let realtime = Realtime::new();
        let meeting_id = MeetingId::new();

        let gone = realtime.subscribe();
        let mut staying = realtime.subscribe();
        drop(gone);

        realtime.publish(&locked(meeting_id));
        assert_eq!(
            staying.recv().await.expect("an event").kind(),
            "meeting.locked"
        );
    }
}
