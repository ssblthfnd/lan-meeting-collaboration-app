//! Carrying domain events to the Host's own window.
//!
//! # The Host is not a client of the LAN server
//!
//! It would have been easy to give the Host UI a WebSocket to `127.0.0.1` and
//! reuse the participant channel. That is rejected, and the reasons are worth
//! writing down (ADR-0018):
//!
//! - The Host window is **in the same process as the database**. A network
//!   socket to reach itself is a round trip through the transport that exists
//!   to face untrusted input, for information it already has the right to read.
//! - It would only work while the LAN server is running. The Host may prepare a
//!   whole meeting - roster, configuration, join token - without ever starting
//!   it (ADR-0016), and the roster must still update as they work.
//! - The LAN server binds a LAN-reachable address. Making the Host's own view
//!   depend on that would tie a local UI concern to a firewall prompt.
//!
//! So the Host receives events over Tauri IPC, through [`TauriEventSink`], and
//! participants receive them over the LAN. One [`app_core::EventSink`] fans out
//! to both, so neither can be told about something the other was not.
//!
//! # The Host's audience is everything
//!
//! No filtering happens here. Every audience includes the Host, because the
//! Host may already read every row of their own database through
//! `HostQueries` (ADR-0014): withholding a *notification* about a row they can
//! read would hide nothing and would only make the roster stale.
//!
//! What is still withheld is **content**. The payload carries identifiers, a
//! version and a timestamp, exactly like the LAN frame; the Host UI re-reads
//! through a command, where the read model decides what a view may hold.

use app_core::event::{DomainEvent, EventSink};
use app_core::id::{MeetingId, NoteId, ParticipantId, SessionId};
use app_core::time::UtcTimestamp;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// The Tauri event name every domain event arrives under.
///
/// One name, with the kind inside the payload, rather than one Tauri event per
/// domain event: a listener that has to subscribe to thirteen names is a
/// listener that will miss the fourteenth.
pub const DOMAIN_EVENT: &str = "domain-event";

/// What the Host UI receives.
///
/// The counterpart of the LAN frame in `app_server::ws`, and deliberately its
/// own type rather than a shared one. They are different audiences, and the
/// house rule is that a shape crossing one boundary does not get reused for
/// another (ADR-0014) - here the difference is `connected`, which only the Host
/// has any use for.
#[derive(Debug, Clone, Serialize)]
pub struct DomainEventDto {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub meeting_id: MeetingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_id: Option<ParticipantId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_id: Option<NoteId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// Present on `presence.changed` alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    pub at: UtcTimestamp,
}

impl From<&DomainEvent> for DomainEventDto {
    fn from(event: &DomainEvent) -> Self {
        let mut dto = Self {
            kind: event.kind(),
            meeting_id: event.meeting_id(),
            participant_id: event.participant_id(),
            session_id: event.session_id(),
            note_id: None,
            version: None,
            connected: None,
            at: event.at(),
        };
        match event {
            DomainEvent::NoteChanged {
                note_id, version, ..
            } => {
                dto.note_id = Some(*note_id);
                dto.version = Some(*version);
            }
            DomainEvent::PresenceChanged { connected, .. } => {
                dto.connected = Some(*connected);
            }
            _ => {}
        }
        dto
    }
}

/// Publishes domain events into the Host window over Tauri IPC.
pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    /// Emit into `app`'s windows.
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl std::fmt::Debug for TauriEventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TauriEventSink").finish_non_exhaustive()
    }
}

impl EventSink for TauriEventSink {
    /// Hand the event to the window, and carry on if it is not there.
    ///
    /// A closing window, a window that has not finished loading, or a
    /// serialization failure are all discarded. The transaction that produced
    /// this event has already committed, and nothing here may suggest otherwise
    /// (architecture rules section 16). The diagnostic goes to the Host's own
    /// console, where every other one goes.
    fn publish(&self, event: &DomainEvent) {
        if let Err(error) = self.app.emit(DOMAIN_EVENT, DomainEventDto::from(event)) {
            eprintln!("[host] a domain event could not reach the window: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(event: &DomainEvent) -> String {
        serde_json::to_string(&DomainEventDto::from(event)).expect("serializes")
    }

    #[test]
    fn a_presence_event_says_which_way_it_went() {
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();

        for connected in [true, false] {
            let payload = json(&DomainEvent::PresenceChanged {
                meeting_id,
                participant_id,
                connected,
                at: UtcTimestamp::now(),
            });
            assert!(
                payload.contains("\"type\":\"presence.changed\""),
                "{payload}"
            );
            assert!(
                payload.contains(&format!("\"connected\":{connected}")),
                "{payload}"
            );
        }
    }

    #[test]
    fn a_note_event_carries_the_version_and_never_the_markdown() {
        let payload = json(&DomainEvent::NoteChanged {
            meeting_id: MeetingId::new(),
            participant_id: ParticipantId::new(),
            note_id: NoteId::new(),
            version: 4,
            at: UtcTimestamp::now(),
        });
        assert!(payload.contains("\"version\":4"), "{payload}");
        assert!(!payload.contains("content"), "{payload}");
        // A note event does not target a socket, so it names no session.
        assert!(!payload.contains("session_id"), "{payload}");
    }

    #[test]
    fn a_lifecycle_event_carries_nothing_it_does_not_need() {
        let payload = json(&DomainEvent::MeetingLocked {
            meeting_id: MeetingId::new(),
            at: UtcTimestamp::now(),
        });
        assert!(payload.contains("\"type\":\"meeting.locked\""), "{payload}");
        for absent in ["participant_id", "session_id", "note_id", "connected"] {
            assert!(!payload.contains(absent), "{payload} carries {absent}");
        }
    }

    #[test]
    fn the_host_payload_never_carries_a_credential() {
        // Same rule as the LAN frame: a Tauri payload is still a payload, and
        // the Host has no more use for a token than a participant does.
        for event in [
            DomainEvent::JoinTokenIssued {
                meeting_id: MeetingId::new(),
                at: UtcTimestamp::now(),
            },
            DomainEvent::SessionRevoked {
                meeting_id: MeetingId::new(),
                participant_id: ParticipantId::new(),
                session_id: SessionId::new(),
                at: UtcTimestamp::now(),
            },
        ] {
            let payload = json(&event);
            assert!(!payload.contains("token\":"), "{payload}");
            assert!(!payload.contains("hash"), "{payload}");
        }
    }
}
