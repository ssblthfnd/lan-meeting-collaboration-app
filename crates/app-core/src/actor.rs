//! Who is performing a mutation.
//!
//! An [`Actor`] is minted at the transport boundary and never parsed from a
//! request body. `app-server` may only mint [`Actor::Participant`] after
//! resolving a stored session token hash; `src-tauri` mints [`Actor::Host`].
//!
//! See the architecture rules, sections 14 and 14.1.

use crate::id::{MeetingId, ParticipantId, SessionId};

/// The authenticated origin of a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// The Host operating the desktop application.
    Host,
    /// A LAN participant, resolved from a validated session token.
    Participant {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        session_id: SessionId,
    },
    /// A remote submission import, confirmed by the Host.
    RemoteImport {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    },
}

impl Actor {
    /// Audit `actor_type` discriminator.
    ///
    /// Mirrors the values required by the architecture rules (sections 17, 18)
    /// and the `CHECK` constraints on `audit_logs.actor_type` and
    /// `note_versions.created_by_type`.
    pub fn actor_type(&self) -> &'static str {
        match self {
            Actor::Host => "HOST",
            Actor::Participant { .. } => "PARTICIPANT",
            Actor::RemoteImport { .. } => "REMOTE_IMPORT",
        }
    }

    /// The participant this actor acts as, if any.
    ///
    /// `None` for the Host, which is what `audit_logs.actor_id` and
    /// `note_versions.created_by` store for Host actions (ADR-0008).
    pub fn participant_id(&self) -> Option<ParticipantId> {
        match self {
            Actor::Host => None,
            Actor::Participant { participant_id, .. }
            | Actor::RemoteImport { participant_id, .. } => Some(*participant_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_type_matches_the_persisted_discriminators() {
        assert_eq!(Actor::Host.actor_type(), "HOST");
        assert_eq!(
            Actor::Participant {
                meeting_id: MeetingId::new(),
                participant_id: ParticipantId::new(),
                session_id: SessionId::new(),
            }
            .actor_type(),
            "PARTICIPANT"
        );
        assert_eq!(
            Actor::RemoteImport {
                meeting_id: MeetingId::new(),
                participant_id: ParticipantId::new(),
            }
            .actor_type(),
            "REMOTE_IMPORT"
        );
    }

    #[test]
    fn only_the_host_has_no_participant_id() {
        // The `created_by_type = 'HOST'` XOR `created_by IS NULL` constraint in
        // the schema depends on exactly this.
        assert!(Actor::Host.participant_id().is_none());

        let participant_id = ParticipantId::new();
        let actor = Actor::RemoteImport {
            meeting_id: MeetingId::new(),
            participant_id,
        };
        assert_eq!(actor.participant_id(), Some(participant_id));
    }
}
