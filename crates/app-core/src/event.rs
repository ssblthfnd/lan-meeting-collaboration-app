//! What happened, and who is allowed to hear about it.
//!
//! A [`DomainEvent`] is a **notification**, not a carrier of state. It says that
//! something committed and names the identifiers needed to go and re-read it. It
//! never carries note content, participant details, a credential or a token
//! hash: a client that wants the new value asks the HTTP or command boundary for
//! it, where the audience rules are enforced again (architecture rules section
//! 16).
//!
//! # Published after the commit, never before
//!
//! ```text
//! mutation -> SQLite transaction -> COMMIT -> DomainEvent -> EventSink::publish
//! ```
//!
//! [`crate::service::Domain`] holds an [`EventSink`] and publishes only on the
//! path where the transaction returned `Ok`. A rolled-back mutation therefore
//! emits nothing, and there is no arrangement of the code where it could: the
//! publish call sits after the `?` on the transaction.
//!
//! The reverse direction is what the architecture forbids. SQLite must not
//! depend on delivery, so [`EventSink::publish`] returns nothing. There is no
//! error for a caller to propagate, which means a disconnected client, a full
//! channel or a closed window cannot roll back a committed write.
//!
//! # Audience is decided here, not in a UI
//!
//! Every variant maps to exactly one [`Audience`] in [`DomainEvent::audience`],
//! which is an exhaustive match. Adding a variant does not compile until its
//! audience has been chosen, so "who may receive this" cannot be left to a
//! filter in a frontend - and a frontend filter would not be enforcement anyway
//! (architecture rules section 14).

use std::sync::Arc;

use crate::id::{MeetingId, NoteId, ParticipantId, SessionId};
use crate::time::UtcTimestamp;

/// Who may receive an event.
///
/// The four values are the four audiences this application actually has. They
/// are ordered below from narrowest reach to widest, which is also the order to
/// prefer when choosing one for a new event: the narrowest audience that makes
/// the feature work is the right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Audience {
    /// The Host alone. No participant socket receives it.
    ///
    /// Roster and lifecycle bookkeeping lands here: it is what the Host's own
    /// window reacts to, and a participant has nothing to do with it.
    HostOnly(MeetingId),

    /// Every live socket of one participant identity, and the Host.
    ///
    /// For something that is about one person - their note, their claim being
    /// acknowledged - and that all of their own devices should see.
    Participant(MeetingId, ParticipantId),

    /// Exactly one socket, and the Host.
    ///
    /// Revocation is session-specific: the Host ends *a session*, not a person.
    /// A participant with a laptop and a phone keeps the other one
    /// (ADR-0002 rule 5). The `SessionId` must come from the authenticated
    /// connection, never from a client frame.
    ParticipantSession(MeetingId, ParticipantId, SessionId),

    /// Every participant in the meeting, and the Host.
    ///
    /// Only for facts that are true for the whole room, such as the meeting
    /// being locked. Anything addressed to a person does not belong here.
    Meeting(MeetingId),
}

impl Audience {
    /// The meeting every audience is scoped to.
    ///
    /// There is no unscoped audience. An event that reached beyond one meeting
    /// would be a cross-meeting leak, and the type has nowhere to express it.
    #[must_use]
    pub const fn meeting_id(&self) -> MeetingId {
        match self {
            Audience::HostOnly(meeting_id)
            | Audience::Meeting(meeting_id)
            | Audience::Participant(meeting_id, _)
            | Audience::ParticipantSession(meeting_id, _, _) => *meeting_id,
        }
    }

    /// Whether the Host receives this event.
    ///
    /// Always. The Host owns the database the event came from and may already
    /// read every row in it (ADR-0014), so withholding a notification would
    /// hide something they can see anyway - and the Host channel is in-process
    /// Tauri IPC, which never touches the LAN (ADR-0018).
    ///
    /// Written as an exhaustive match rather than `true` so that a new variant
    /// of [`Audience`] has to be considered here too.
    #[must_use]
    pub const fn reaches_host(&self) -> bool {
        match self {
            Audience::HostOnly(_)
            | Audience::Meeting(_)
            | Audience::Participant(_, _)
            | Audience::ParticipantSession(_, _, _) => true,
        }
    }

    /// Whether one authenticated participant socket receives this event.
    ///
    /// The three arguments describe the **connection**, and a connection's
    /// identity is built from its session row at the upgrade and never changes
    /// (architecture rules section 14.1). Nothing a client sends reaches this
    /// function, so a browser cannot widen its own audience by asking.
    #[must_use]
    pub fn reaches_session(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        session_id: SessionId,
    ) -> bool {
        match self {
            // The one audience that stops at the Host.
            Audience::HostOnly(_) => false,
            Audience::Meeting(target) => *target == meeting_id,
            Audience::Participant(target, participant) => {
                *target == meeting_id && *participant == participant_id
            }
            Audience::ParticipantSession(target, participant, session) => {
                *target == meeting_id && *participant == participant_id && *session == session_id
            }
        }
    }
}

/// Something that committed.
///
/// Every variant carries identifiers, a version where one exists, and a
/// timestamp. None carries content, and none carries a credential - a property
/// a test in this module asserts against the `Debug` output, because `{:?}` is
/// how a value ends up in a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    /// A meeting was created, in `DRAFT`.
    MeetingCreated {
        meeting_id: MeetingId,
        at: UtcTimestamp,
    },
    /// A `DRAFT` meeting's configuration was replaced.
    MeetingUpdated {
        meeting_id: MeetingId,
        at: UtcTimestamp,
    },
    /// A meeting moved from `DRAFT` to `OPEN`.
    MeetingOpened {
        meeting_id: MeetingId,
        at: UtcTimestamp,
    },
    /// A meeting moved from `OPEN` to `LOCKED`.
    ///
    /// The one lifecycle event participants receive, because it is the one that
    /// changes what their screen should offer. It does not change what they are
    /// *allowed* to do - that was already re-read inside every mutating
    /// transaction (architecture rules section 15) - so a participant who never
    /// receives this is refused exactly as firmly as one who does.
    MeetingLocked {
        meeting_id: MeetingId,
        at: UtcTimestamp,
    },
    /// A join token was minted, invalidating any previous one.
    ///
    /// Carries neither the token nor its hash. The Host is told that the link
    /// changed; the link itself travels in the command response that asked for
    /// it, once (PRD 22.2).
    JoinTokenIssued {
        meeting_id: MeetingId,
        at: UtcTimestamp,
    },
    /// A participant was added to a `DRAFT` roster.
    ParticipantAdded {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        at: UtcTimestamp,
    },
    /// A participant's details were replaced.
    ParticipantUpdated {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        at: UtcTimestamp,
    },
    /// A participant was removed from a `DRAFT` roster.
    ParticipantRemoved {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        at: UtcTimestamp,
    },
    /// An identity was bound to a new session (ADR-0002).
    ///
    /// Deliberately carries no `session_id`: the Host needs to know someone
    /// joined, and a session identifier is not part of that fact.
    IdentityClaimed {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        at: UtcTimestamp,
    },
    /// The Host acknowledged a claim.
    ///
    /// Grants nothing. It is display state, and it is the participant's own, so
    /// it goes to them and to nobody else (ADR-0016).
    ClaimApproved {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        at: UtcTimestamp,
    },
    /// A session was ended by the Host.
    ///
    /// The only variant carrying a `SessionId`, and the reason the
    /// [`Audience::ParticipantSession`] audience exists: revocation targets one
    /// session, so exactly one socket closes and the participant's other
    /// devices keep working.
    SessionRevoked {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        session_id: SessionId,
        at: UtcTimestamp,
    },
    /// A participant's live socket count crossed zero.
    ///
    /// Emitted on `0 -> 1` and `1 -> 0` only, never per socket and never per
    /// keepalive, so a second tab cannot make the roster flicker. Presence is
    /// an observation and grants no authority whatsoever (ADR-0018).
    PresenceChanged {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        connected: bool,
        at: UtcTimestamp,
    },
    /// A note was written, and a history row was appended.
    ///
    /// Carries the version, never the Markdown. A client that may read the note
    /// re-fetches it through the boundary that decides whether it may
    /// (architecture rules section 16).
    NoteChanged {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        note_id: NoteId,
        version: i64,
        at: UtcTimestamp,
    },
}

impl DomainEvent {
    /// How many variants exist.
    ///
    /// Paired with [`DomainEvent::variant_index`] to make the audience table in
    /// this module's tests exhaustive: adding a variant forces a new index,
    /// which forces this constant up, which forces a sample and an explicit
    /// audience assertion.
    pub const VARIANT_COUNT: usize = 13;

    /// A stable position per variant, for exhaustiveness checking only.
    ///
    /// Not a wire value and not persisted - [`DomainEvent::kind`] is the name
    /// that crosses a boundary. Renumbering this breaks nothing but the test
    /// that uses it.
    #[must_use]
    pub const fn variant_index(&self) -> usize {
        match self {
            DomainEvent::MeetingCreated { .. } => 0,
            DomainEvent::MeetingUpdated { .. } => 1,
            DomainEvent::MeetingOpened { .. } => 2,
            DomainEvent::MeetingLocked { .. } => 3,
            DomainEvent::JoinTokenIssued { .. } => 4,
            DomainEvent::ParticipantAdded { .. } => 5,
            DomainEvent::ParticipantUpdated { .. } => 6,
            DomainEvent::ParticipantRemoved { .. } => 7,
            DomainEvent::IdentityClaimed { .. } => 8,
            DomainEvent::ClaimApproved { .. } => 9,
            DomainEvent::SessionRevoked { .. } => 10,
            DomainEvent::PresenceChanged { .. } => 11,
            DomainEvent::NoteChanged { .. } => 12,
        }
    }

    /// The discriminator that crosses a transport boundary.
    ///
    /// Shared verbatim with `packages/contracts/src/ws-events.ts`. Dotted
    /// `subject.verb`, which is the vocabulary the architecture rules use for
    /// events and reads the same in both languages.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            DomainEvent::MeetingCreated { .. } => "meeting.created",
            DomainEvent::MeetingUpdated { .. } => "meeting.updated",
            DomainEvent::MeetingOpened { .. } => "meeting.opened",
            DomainEvent::MeetingLocked { .. } => "meeting.locked",
            DomainEvent::JoinTokenIssued { .. } => "meeting.join_token_issued",
            DomainEvent::ParticipantAdded { .. } => "participant.added",
            DomainEvent::ParticipantUpdated { .. } => "participant.updated",
            DomainEvent::ParticipantRemoved { .. } => "participant.removed",
            DomainEvent::IdentityClaimed { .. } => "participant.claimed",
            DomainEvent::ClaimApproved { .. } => "claim.approved",
            DomainEvent::SessionRevoked { .. } => "session.revoked",
            DomainEvent::PresenceChanged { .. } => "presence.changed",
            DomainEvent::NoteChanged { .. } => "note.changed",
        }
    }

    /// Who may receive this event.
    ///
    /// The whole audience policy of the application, in one exhaustive match.
    /// The rule behind the table: a participant is told what changes their own
    /// screen and nothing else, and everything about the roster, the lifecycle
    /// before it is locked, and who is connected is the Host's.
    #[must_use]
    pub const fn audience(&self) -> Audience {
        match self {
            // Roster and preparation. A meeting is configured and its roster
            // settled while it is DRAFT (ADR-0013), when by definition no
            // participant has joined - so there is nobody else to tell.
            DomainEvent::MeetingCreated { meeting_id, .. }
            | DomainEvent::MeetingUpdated { meeting_id, .. }
            | DomainEvent::MeetingOpened { meeting_id, .. }
            | DomainEvent::JoinTokenIssued { meeting_id, .. }
            | DomainEvent::ParticipantAdded { meeting_id, .. }
            | DomainEvent::ParticipantUpdated { meeting_id, .. }
            | DomainEvent::ParticipantRemoved { meeting_id, .. } => Audience::HostOnly(*meeting_id),

            // Who has joined and who is connected is the Host's operational
            // view of their own room. A participant has no screen for it, and
            // an event nobody displays is an event that can only leak.
            DomainEvent::IdentityClaimed { meeting_id, .. }
            | DomainEvent::PresenceChanged { meeting_id, .. } => Audience::HostOnly(*meeting_id),

            // True for the whole room.
            DomainEvent::MeetingLocked { meeting_id, .. } => Audience::Meeting(*meeting_id),

            // About one person, on all of their devices.
            DomainEvent::ClaimApproved {
                meeting_id,
                participant_id,
                ..
            }
            | DomainEvent::NoteChanged {
                meeting_id,
                participant_id,
                ..
            } => Audience::Participant(*meeting_id, *participant_id),

            // About one session.
            DomainEvent::SessionRevoked {
                meeting_id,
                participant_id,
                session_id,
                ..
            } => Audience::ParticipantSession(*meeting_id, *participant_id, *session_id),
        }
    }

    /// The meeting this event belongs to.
    #[must_use]
    pub const fn meeting_id(&self) -> MeetingId {
        self.audience().meeting_id()
    }

    /// When it committed.
    #[must_use]
    pub const fn at(&self) -> UtcTimestamp {
        match self {
            DomainEvent::MeetingCreated { at, .. }
            | DomainEvent::MeetingUpdated { at, .. }
            | DomainEvent::MeetingOpened { at, .. }
            | DomainEvent::MeetingLocked { at, .. }
            | DomainEvent::JoinTokenIssued { at, .. }
            | DomainEvent::ParticipantAdded { at, .. }
            | DomainEvent::ParticipantUpdated { at, .. }
            | DomainEvent::ParticipantRemoved { at, .. }
            | DomainEvent::IdentityClaimed { at, .. }
            | DomainEvent::ClaimApproved { at, .. }
            | DomainEvent::SessionRevoked { at, .. }
            | DomainEvent::PresenceChanged { at, .. }
            | DomainEvent::NoteChanged { at, .. } => *at,
        }
    }

    /// The participant this event is about, if any.
    #[must_use]
    pub const fn participant_id(&self) -> Option<ParticipantId> {
        match self {
            DomainEvent::MeetingCreated { .. }
            | DomainEvent::MeetingUpdated { .. }
            | DomainEvent::MeetingOpened { .. }
            | DomainEvent::MeetingLocked { .. }
            | DomainEvent::JoinTokenIssued { .. } => None,

            DomainEvent::ParticipantAdded { participant_id, .. }
            | DomainEvent::ParticipantUpdated { participant_id, .. }
            | DomainEvent::ParticipantRemoved { participant_id, .. }
            | DomainEvent::IdentityClaimed { participant_id, .. }
            | DomainEvent::ClaimApproved { participant_id, .. }
            | DomainEvent::SessionRevoked { participant_id, .. }
            | DomainEvent::PresenceChanged { participant_id, .. }
            | DomainEvent::NoteChanged { participant_id, .. } => Some(*participant_id),
        }
    }

    /// The session this event is about, if it is about one.
    ///
    /// `Some` for exactly one variant. A transport uses it to close the right
    /// socket; nothing else has a reason to read it.
    #[must_use]
    pub const fn session_id(&self) -> Option<SessionId> {
        match self {
            DomainEvent::SessionRevoked { session_id, .. } => Some(*session_id),
            _ => None,
        }
    }
}

/// Where a published [`DomainEvent`] goes.
///
/// Deliberately infallible. Delivery is best-effort by design: a WebSocket that
/// went away, a channel with no receiver and a closed Host window are all
/// ordinary conditions, and none of them may affect a transaction that has
/// already committed (architecture rules section 16). An implementation that
/// wants to record a failure logs it on the Host's own console.
///
/// `Send + Sync` because one sink is shared by the Tauri command thread, the
/// LAN server's Tokio workers and the blocking pool.
pub trait EventSink: Send + Sync {
    /// Hand the event on. Never blocks on a client, never fails.
    fn publish(&self, event: &DomainEvent);
}

/// A sink that drops everything.
///
/// The default for a [`crate::service::Domain`] built without one, which is
/// what every domain and persistence test uses: those tests are about what
/// SQLite holds afterwards, and wiring a channel into them would test the
/// channel instead.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEvents;

impl EventSink for NoEvents {
    fn publish(&self, _event: &DomainEvent) {}
}

/// Fan one event out to several sinks.
///
/// This is how the Host and the LAN receive the same event without the domain
/// knowing either of them exists:
///
/// ```text
/// Domain
///  └── EventSink (composite)
///       ├── LAN broadcast   -> participant WebSockets
///       └── Tauri emitter   -> the Host window, in process
/// ```
///
/// The Host is deliberately **not** a WebSocket client of the LAN server
/// (ADR-0018). It is in the same process as the database; giving it a network
/// socket would put the Host's own view on the transport that faces untrusted
/// input, for no gain.
pub struct CompositeSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl CompositeSink {
    /// Fan out to each sink in turn, in order.
    #[must_use]
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        Self { sinks }
    }
}

impl std::fmt::Debug for CompositeSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeSink")
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

impl EventSink for CompositeSink {
    /// One sink's problem is not another's.
    ///
    /// Every sink is offered the event, and since [`EventSink::publish`] cannot
    /// fail there is no early return here that could silently skip one.
    fn publish(&self, event: &DomainEvent) {
        for sink in &self.sinks {
            sink.publish(event);
        }
    }
}

impl<T: EventSink + ?Sized> EventSink for Arc<T> {
    fn publish(&self, event: &DomainEvent) {
        (**self).publish(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// One sample of every variant, next to the audience it must have.
    ///
    /// The audience policy of the whole application, written out. Changing a
    /// line here is changing who receives something, which is exactly the kind
    /// of change that should be visible in a diff.
    fn every_variant() -> Vec<(DomainEvent, Audience)> {
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();
        let session_id = SessionId::new();
        let at = UtcTimestamp::now();

        vec![
            (
                DomainEvent::MeetingCreated { meeting_id, at },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::MeetingUpdated { meeting_id, at },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::MeetingOpened { meeting_id, at },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::MeetingLocked { meeting_id, at },
                Audience::Meeting(meeting_id),
            ),
            (
                DomainEvent::JoinTokenIssued { meeting_id, at },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::ParticipantAdded {
                    meeting_id,
                    participant_id,
                    at,
                },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::ParticipantUpdated {
                    meeting_id,
                    participant_id,
                    at,
                },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::ParticipantRemoved {
                    meeting_id,
                    participant_id,
                    at,
                },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::IdentityClaimed {
                    meeting_id,
                    participant_id,
                    at,
                },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::ClaimApproved {
                    meeting_id,
                    participant_id,
                    at,
                },
                Audience::Participant(meeting_id, participant_id),
            ),
            (
                DomainEvent::SessionRevoked {
                    meeting_id,
                    participant_id,
                    session_id,
                    at,
                },
                Audience::ParticipantSession(meeting_id, participant_id, session_id),
            ),
            (
                DomainEvent::PresenceChanged {
                    meeting_id,
                    participant_id,
                    connected: true,
                    at,
                },
                Audience::HostOnly(meeting_id),
            ),
            (
                DomainEvent::NoteChanged {
                    meeting_id,
                    participant_id,
                    note_id: NoteId::new(),
                    version: 3,
                    at,
                },
                Audience::Participant(meeting_id, participant_id),
            ),
        ]
    }

    #[test]
    fn every_event_variant_has_an_explicit_audience() {
        // The exhaustiveness guard. A new variant must add an arm to
        // `variant_index` to compile at all; that arm needs an unused index,
        // which needs `VARIANT_COUNT` raised, which fails this test until a
        // sample and its audience are written down below.
        let samples = every_variant();
        assert_eq!(
            samples.len(),
            DomainEvent::VARIANT_COUNT,
            "every DomainEvent variant needs a sample and an explicit audience"
        );

        let mut indices = HashSet::new();
        for (event, expected) in samples {
            assert!(
                event.variant_index() < DomainEvent::VARIANT_COUNT,
                "{event:?} has an index past VARIANT_COUNT"
            );
            assert!(
                indices.insert(event.variant_index()),
                "two samples share a variant index: {event:?}"
            );
            assert_eq!(event.audience(), expected, "{event:?}");
        }
        assert_eq!(indices.len(), DomainEvent::VARIANT_COUNT);
    }

    #[test]
    fn every_kind_is_distinct() {
        // The kind is what crosses the boundary and what a client switches on.
        // Two variants sharing one would make an event silently ambiguous.
        let kinds: HashSet<&str> = every_variant()
            .iter()
            .map(|(event, _)| event.kind())
            .collect();
        assert_eq!(kinds.len(), DomainEvent::VARIANT_COUNT);
    }

    #[test]
    fn a_host_only_event_reaches_no_participant() {
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();
        let session_id = SessionId::new();

        let audience = Audience::HostOnly(meeting_id);
        assert!(audience.reaches_host());
        assert!(!audience.reaches_session(meeting_id, participant_id, session_id));
    }

    #[test]
    fn a_participant_never_receives_another_participants_event() {
        // The property the whole audience model exists for.
        let meeting_id = MeetingId::new();
        let alice = ParticipantId::new();
        let bob = ParticipantId::new();
        let alice_session = SessionId::new();
        let bob_session = SessionId::new();

        let alices = Audience::Participant(meeting_id, alice);

        assert!(alices.reaches_session(meeting_id, alice, alice_session));
        assert!(!alices.reaches_session(meeting_id, bob, bob_session));
    }

    #[test]
    fn revocation_reaches_one_session_and_leaves_the_others_alone() {
        // A participant with a laptop and a phone: revoking the laptop must not
        // close the phone (ADR-0002 rule 5).
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();
        let laptop = SessionId::new();
        let phone = SessionId::new();

        let audience = Audience::ParticipantSession(meeting_id, participant_id, laptop);

        assert!(audience.reaches_session(meeting_id, participant_id, laptop));
        assert!(!audience.reaches_session(meeting_id, participant_id, phone));
    }

    #[test]
    fn no_audience_crosses_a_meeting_boundary() {
        // A session of meeting A must never be handed anything from meeting B,
        // whatever the audience.
        let a = MeetingId::new();
        let b = MeetingId::new();
        let participant_id = ParticipantId::new();
        let session_id = SessionId::new();

        for audience in [
            Audience::HostOnly(a),
            Audience::Meeting(a),
            Audience::Participant(a, participant_id),
            Audience::ParticipantSession(a, participant_id, session_id),
        ] {
            assert_eq!(audience.meeting_id(), a);
            assert!(
                !audience.reaches_session(b, participant_id, session_id),
                "{audience:?} leaked into another meeting"
            );
        }
    }

    #[test]
    fn a_meeting_wide_event_reaches_every_participant_of_that_meeting() {
        let meeting_id = MeetingId::new();
        let audience = Audience::Meeting(meeting_id);

        for _ in 0..3 {
            assert!(audience.reaches_session(meeting_id, ParticipantId::new(), SessionId::new()));
        }
    }

    #[test]
    fn only_revocation_names_a_session() {
        // §4 of the event contract: a session id travels only where closing the
        // right socket requires it.
        for (event, _) in every_variant() {
            let expected = matches!(event, DomainEvent::SessionRevoked { .. });
            assert_eq!(event.session_id().is_some(), expected, "{event:?}");
        }
    }

    #[test]
    fn no_event_can_carry_content_or_a_credential() {
        // `{:?}` is how a value reaches a log line or a panic message. The
        // guarantee is structural - no variant has a field for content, a
        // token or a hash - and this asserts the structure has not drifted.
        //
        // Field names, with the colon `Debug` prints after them: the variant
        // *named* `JoinTokenIssued` is the point, since announcing that a token
        // was issued is exactly what it may do without carrying one.
        for (event, _) in every_variant() {
            let debugged = format!("{event:?}");
            for forbidden in ["token:", "hash:", "content:", "secret:", "password:"] {
                assert!(
                    !debugged.to_lowercase().contains(forbidden),
                    "{debugged} carries a `{forbidden}` field"
                );
            }

            // Nothing shaped like a stored credential either: a token and its
            // hash are both 64 lowercase hexadecimal characters (ADR-0011).
            let hexadecimal: String = debugged
                .chars()
                .map(|c| {
                    if c.is_ascii_digit() || ('a'..='f').contains(&c) {
                        c
                    } else {
                        ' '
                    }
                })
                .collect();
            assert!(
                hexadecimal.split_whitespace().all(|run| run.len() < 64),
                "{debugged} contains something shaped like a credential"
            );
        }
    }

    /// A sink that records what it was given.
    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<&'static str>>,
    }

    impl EventSink for Recorder {
        fn publish(&self, event: &DomainEvent) {
            self.seen.lock().expect("not poisoned").push(event.kind());
        }
    }

    /// A sink whose delivery always fails, without saying so.
    ///
    /// The shape every real sink has: a broadcast with no receivers, a window
    /// that has closed. `publish` returns nothing, so there is no failure for a
    /// caller to react to - which is the point.
    #[derive(Default)]
    struct AlwaysFails {
        attempts: Mutex<usize>,
    }

    impl EventSink for AlwaysFails {
        fn publish(&self, _event: &DomainEvent) {
            *self.attempts.lock().expect("not poisoned") += 1;
        }
    }

    #[test]
    fn a_composite_offers_the_event_to_every_sink_even_when_one_fails() {
        let failing = Arc::new(AlwaysFails::default());
        let recorder = Arc::new(Recorder::default());

        let composite = CompositeSink::new(vec![
            Arc::clone(&failing) as Arc<dyn EventSink>,
            Arc::clone(&recorder) as Arc<dyn EventSink>,
        ]);

        let event = DomainEvent::MeetingLocked {
            meeting_id: MeetingId::new(),
            at: UtcTimestamp::now(),
        };
        composite.publish(&event);

        // The failing sink did not stop the one behind it.
        assert_eq!(*failing.attempts.lock().expect("not poisoned"), 1);
        assert_eq!(
            *recorder.seen.lock().expect("not poisoned"),
            vec!["meeting.locked"]
        );
    }

    #[test]
    fn dropping_every_event_is_a_valid_sink() {
        // What a domain built without a sink uses, and what makes realtime
        // optional rather than load-bearing.
        NoEvents.publish(&DomainEvent::MeetingCreated {
            meeting_id: MeetingId::new(),
            at: UtcTimestamp::now(),
        });
    }
}
