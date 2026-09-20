//! The participant roster.
//!
//! A participant is an identity the Host prepares in advance: a name, and
//! optionally a department, position and role in the meeting (PRD section 7).
//! It is not an account, and it carries no credential. A LAN participant later
//! *claims* one of these identities, and the credential produced by that claim
//! lives in `participant_sessions` (ADR-0002) - not here.
//!
//! The roster is decided while the meeting is `DRAFT` and fixed once it opens
//! (ADR-0013). That rule is enforced in [`crate::service::Domain`], because it
//! needs the meeting's current status; this module holds only the facts about a
//! participant that can be judged from the input alone.

use crate::error::{DomainError, DomainResult};
use crate::text;

/// Most participants a meeting may have (PRD section 7).
///
/// The limit is enforced twice on purpose: [`crate::service::Domain`] counts the
/// roster inside the mutating transaction and refuses the one that would exceed
/// it, and a trigger in the migration refuses it again. The same reasoning as
/// the five-link cap (ADR-0011): a limit that lives only in Rust is a limit some
/// future code path can skip.
pub const MAX_PARTICIPANTS: usize = 99;

/// Fewest participants a meeting is meant to have (PRD section 7).
///
/// Recorded here for completeness. Nothing enforces it yet: the step that
/// enforces it is the one that decides *when* it applies, which can only be at
/// the point a meeting opens - a meeting under construction necessarily passes
/// through an empty roster.
pub const MIN_PARTICIPANTS: usize = 1;

/// What the Host records about one participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantDetails {
    /// How the participant is identified to themselves and to the Host.
    ///
    /// Required. Deliberately **not** unique within a meeting: neither the PRD
    /// nor the schema makes it unique, and two people genuinely can share a
    /// name. Identity is the participant id, never the name - a name in a
    /// request or a submission file is data, not authority (architecture rules
    /// sections 10 and 14.1).
    pub name: String,
    pub department: Option<String>,
    pub position: Option<String>,
    pub meeting_role: Option<String>,
}

impl ParticipantDetails {
    /// Validate and normalise, returning the form that is written to SQLite.
    pub fn validated(&self) -> DomainResult<Self> {
        Ok(Self {
            name: text::required("participant name", &self.name)?,
            department: text::optional(self.department.as_deref()),
            position: text::optional(self.position.as_deref()),
            meeting_role: text::optional(self.meeting_role.as_deref()),
        })
    }
}

/// Refuse the addition that would take a roster past [`MAX_PARTICIPANTS`].
///
/// `current` is read inside the mutating transaction, never from a cached count
/// or from a UI (architecture rules section 15). The refusal names the limit and
/// the roster size that was found, so the Host is told why rather than merely
/// that it failed (section 22).
pub fn ensure_room_for_one_more(current: i64) -> DomainResult<()> {
    if current >= MAX_PARTICIPANTS as i64 {
        return Err(DomainError::Validation {
            field: "participant count",
            expected: "at most 99 participants per meeting",
            detected: format!("the meeting already has {current}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details(name: &str) -> ParticipantDetails {
        ParticipantDetails {
            name: name.to_owned(),
            department: None,
            position: None,
            meeting_role: None,
        }
    }

    #[test]
    fn details_are_stored_trimmed_with_blank_optional_fields_absent() {
        let input = ParticipantDetails {
            name: "  Budi Santoso ".to_owned(),
            department: Some("  Finance  ".to_owned()),
            position: Some("   ".to_owned()),
            meeting_role: None,
        };

        let normalised = input.validated().unwrap();
        assert_eq!(normalised.name, "Budi Santoso");
        assert_eq!(normalised.department, Some("Finance".to_owned()));
        assert_eq!(normalised.position, None);
        assert_eq!(normalised.meeting_role, None);
    }

    #[test]
    fn a_participant_must_have_a_name() {
        for blank in ["", "   ", "\n\t"] {
            let err = details(blank).validated().unwrap_err();
            assert!(
                matches!(
                    err,
                    DomainError::Validation {
                        field: "participant name",
                        ..
                    }
                ),
                "{err:?}"
            );
        }
    }

    #[test]
    fn two_participants_may_share_a_name() {
        // Not an oversight: nothing in the PRD or the schema makes a name
        // unique, and identity is the participant id.
        assert_eq!(
            details("Budi Santoso").validated().unwrap(),
            details("Budi Santoso").validated().unwrap()
        );
    }

    #[test]
    fn the_roster_limit_is_ninety_nine() {
        assert_eq!(MAX_PARTICIPANTS, 99);
        assert!(ensure_room_for_one_more(0).is_ok());
        assert!(ensure_room_for_one_more(98).is_ok());

        let err = ensure_room_for_one_more(99).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("at most 99"), "{message}");
        assert!(message.contains("already has 99"), "{message}");
    }
}
