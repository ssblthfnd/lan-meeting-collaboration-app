//! Who is performing a mutation.
//!
//! An [`Actor`] is minted at the transport boundary and never parsed from a
//! request body. `app-server` may only mint [`Actor::Participant`] after
//! resolving a stored session token hash; `src-tauri` mints [`Actor::Host`].
//!
//! See the architecture rules, sections 14 and 14.1.
//!
//! # Two LAN contexts, two variants
//!
//! A browser arrives holding one of two credentials, and they are not the same
//! kind of thing:
//!
//! | Credential | Variant | Proves |
//! | --- | --- | --- |
//! | join token | [`Actor::Claimant`] | they were given the meeting's URL |
//! | session token | [`Actor::Participant`] | they already claimed an identity |
//!
//! [`Actor::Claimant`] exists because a claim happens *before* any session does
//! (ADR-0016). Reusing [`Actor::Participant`] for it would mean inventing a
//! session id that does not exist, inside the very type whose purpose is to say
//! "this request was authenticated". Keeping them apart makes "holding a join
//! URL does not let you write a note" a fact the compiler checks rather than one
//! a reviewer has to notice.

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
    /// Someone holding a valid join token, claiming an identity.
    ///
    /// The narrowest actor there is: it may claim the one identity it names and
    /// nothing else. `participant_id` here is the identity being *requested* -
    /// a target, not an assertion of who is asking - and the database decides
    /// who wins a race for it (ADR-0002, ADR-0016).
    Claimant {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
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
            // A claim is performed by a participant. Recording it as anything
            // else would need a fourth value in the `audit_logs.actor_type` and
            // `note_versions.created_by_type` CHECK constraints - a migration,
            // and a split in a vocabulary two ADRs already depend on - to say
            // something the existing word already says (ADR-0016).
            Actor::Participant { .. } | Actor::Claimant { .. } => "PARTICIPANT",
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
            | Actor::Claimant { participant_id, .. }
            | Actor::RemoteImport { participant_id, .. } => Some(*participant_id),
        }
    }

    /// The meeting this actor is confined to, if any.
    ///
    /// `None` for the Host, who is a party to every meeting in their own
    /// database. Every other actor is bound to exactly one (ADR-0002).
    #[must_use]
    pub fn meeting_id(&self) -> Option<MeetingId> {
        match self {
            Actor::Host => None,
            Actor::Participant { meeting_id, .. }
            | Actor::Claimant { meeting_id, .. }
            | Actor::RemoteImport { meeting_id, .. } => Some(*meeting_id),
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
    fn a_claimant_is_audited_as_the_participant_it_is() {
        // Deliberately the same discriminator as a session-backed participant.
        // A fourth value would mean a migration on two CHECK constraints to
        // express something "PARTICIPANT" already expresses (ADR-0016).
        let actor = Actor::Claimant {
            meeting_id: MeetingId::new(),
            participant_id: ParticipantId::new(),
        };
        assert_eq!(actor.actor_type(), "PARTICIPANT");
        assert_eq!(actor.actor_type(), {
            Actor::Participant {
                meeting_id: MeetingId::new(),
                participant_id: ParticipantId::new(),
                session_id: SessionId::new(),
            }
            .actor_type()
        });
    }

    #[test]
    fn every_actor_but_the_host_is_confined_to_one_meeting() {
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();

        assert_eq!(Actor::Host.meeting_id(), None);
        assert_eq!(
            Actor::Claimant {
                meeting_id,
                participant_id,
            }
            .meeting_id(),
            Some(meeting_id)
        );
        assert_eq!(
            Actor::Participant {
                meeting_id,
                participant_id,
                session_id: SessionId::new(),
            }
            .meeting_id(),
            Some(meeting_id)
        );
        assert_eq!(
            Actor::RemoteImport {
                meeting_id,
                participant_id,
            }
            .meeting_id(),
            Some(meeting_id)
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
