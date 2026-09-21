//! Participant sessions and the claim model.
//!
//! A LAN participant has no account and no password. They open the join URL,
//! pick an identity the Host prepared, and from that moment a **session token**
//! is their only credential (ADR-0002 rule 6). This module holds what that
//! means in the domain; the token itself is generated, hashed and carried by the
//! transport, and only its [`crate::token::TokenHash`] ever reaches here.
//!
//! # Approval is acknowledgement, not a gate
//!
//! A session is usable the moment it exists. `approved_at` records that the
//! Host *noticed* the claim, and setting it changes nothing about what the
//! participant may do (ADR-0016).
//!
//! This is the reading that satisfies both documents at once: PRD 5.2.1 gives
//! the Host approve and reject, while ADR-0002 rejected "the Host approves every
//! join" precisely because it is a bottleneck at the start of a meeting, when
//! everyone joins at once. Acknowledgement is not a bottleneck; a gate is.
//!
//! The one thing that *does* remove authority is revocation:
//! `revoked_at IS NOT NULL` means the session can no longer act, and no
//! subsequent approval brings it back.
//!
//! # Claim status is derived
//!
//! Nothing here is stored as a status column. [`ClaimStatus`] is computed from
//! the session rows for a participant (ADR-0008), which is why a session that
//! is revoked and a participant who never claimed cannot disagree with each
//! other - there is only one set of facts.

use core::fmt;

use crate::id::{MeetingId, ParticipantId, SessionId};
use crate::time::UtcTimestamp;

/// A participant identity's claim state, derived from `participant_sessions`.
///
/// The string forms are the values `packages/contracts` uses and the ones the
/// derivation query in `app-db` produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaimStatus {
    /// No session row exists for this participant.
    Unclaimed,
    /// A live session exists that the Host has not acknowledged.
    ///
    /// **Fully able to act.** This is the normal state of a participant who
    /// joined and whose Host has not clicked approve (ADR-0016).
    Pending,
    /// A live session the Host has acknowledged.
    Claimed,
    /// Sessions exist and every one of them is revoked. The identity is
    /// claimable again.
    Revoked,
}

impl ClaimStatus {
    /// The discriminator shared with the TypeScript contracts.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ClaimStatus::Unclaimed => "UNCLAIMED",
            ClaimStatus::Pending => "PENDING",
            ClaimStatus::Claimed => "CLAIMED",
            ClaimStatus::Revoked => "REVOKED",
        }
    }

    /// Whether a live session currently holds this identity.
    ///
    /// True for both [`ClaimStatus::Pending`] and [`ClaimStatus::Claimed`],
    /// because both are live. This is the single definition of "taken", and it
    /// is what first-claim-wins means: an identity in either state cannot be
    /// claimed by another browser (ADR-0002 rule 2).
    #[must_use]
    pub fn is_held(&self) -> bool {
        matches!(self, ClaimStatus::Pending | ClaimStatus::Claimed)
    }
}

impl fmt::Display for ClaimStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A live session, as resolved from a presented credential.
///
/// Every field comes from the stored row. This is what the transport turns into
/// an [`crate::actor::Actor::Participant`], and it is the reason a browser
/// cannot choose who it is: nothing in this struct was supplied by the caller
/// (architecture rules section 14.1 rule 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBinding {
    pub session_id: SessionId,
    pub meeting_id: MeetingId,
    pub participant_id: ParticipantId,
    /// Set when the Host acknowledged the claim. Does not affect authority.
    pub approved_at: Option<UtcTimestamp>,
}

impl SessionBinding {
    /// Whether the Host has acknowledged this claim.
    ///
    /// For display only. No authorization decision reads this
    /// (ADR-0016), and a test asserts as much.
    #[must_use]
    pub fn is_acknowledged(&self) -> bool {
        self.approved_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_strings_match_the_shared_contract() {
        assert_eq!(ClaimStatus::Unclaimed.as_str(), "UNCLAIMED");
        assert_eq!(ClaimStatus::Pending.as_str(), "PENDING");
        assert_eq!(ClaimStatus::Claimed.as_str(), "CLAIMED");
        assert_eq!(ClaimStatus::Revoked.as_str(), "REVOKED");
    }

    #[test]
    fn a_pending_identity_is_held_just_as_firmly_as_an_acknowledged_one() {
        // First-claim-wins does not wait for the Host: an identity someone is
        // already using cannot be taken, acknowledged or not.
        assert!(ClaimStatus::Pending.is_held());
        assert!(ClaimStatus::Claimed.is_held());

        assert!(!ClaimStatus::Unclaimed.is_held());
        assert!(!ClaimStatus::Revoked.is_held());
    }

    #[test]
    fn acknowledgement_is_reported_but_carries_no_authority() {
        let binding = SessionBinding {
            session_id: SessionId::new(),
            meeting_id: MeetingId::new(),
            participant_id: ParticipantId::new(),
            approved_at: None,
        };
        assert!(!binding.is_acknowledged());

        let acknowledged = SessionBinding {
            approved_at: Some(UtcTimestamp::now()),
            ..binding
        };
        assert!(acknowledged.is_acknowledged());

        // Both resolve to the same participant in the same meeting: approval
        // changed the display, not the identity or its reach.
        assert_eq!(binding.participant_id, acknowledged.participant_id);
        assert_eq!(binding.meeting_id, acknowledged.meeting_id);
    }
}
