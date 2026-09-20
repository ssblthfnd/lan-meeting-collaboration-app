//! Meeting lifecycle.
//!
//! `DRAFT -> OPEN -> LOCKED`, exactly as PRD section 6 defines it and no
//! further. `EXPORTED` is deliberately not a status: the PRD says export is
//! recorded in the audit log and the meeting stays `LOCKED`.
//!
//! There is no unlock. Nothing in the PRD or the ADRs defines one, and a lock
//! that can be undone is not the guarantee PRD section 19 describes.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};
use crate::id::MeetingId;

/// Where a meeting is in its lifecycle.
///
/// The string forms are the values stored in `meetings.status` and pinned by a
/// `CHECK` constraint in the migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MeetingStatus {
    /// The Host is still preparing the meeting.
    Draft,
    /// Participants may join and write notes.
    Open,
    /// No further changes are permitted.
    Locked,
}

impl MeetingStatus {
    /// The persisted discriminator.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            MeetingStatus::Draft => "DRAFT",
            MeetingStatus::Open => "OPEN",
            MeetingStatus::Locked => "LOCKED",
        }
    }

    /// Whether the meeting still accepts mutations.
    ///
    /// This is the single definition of "mutable". Callers ask the meeting,
    /// rather than each deciding for themselves what locked means.
    #[must_use]
    pub fn accepts_mutations(&self) -> bool {
        !matches!(self, MeetingStatus::Locked)
    }
}

impl fmt::Display for MeetingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MeetingStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DRAFT" => Ok(MeetingStatus::Draft),
            "OPEN" => Ok(MeetingStatus::Open),
            "LOCKED" => Ok(MeetingStatus::Locked),
            other => Err(DomainError::Validation {
                field: "meeting status",
                expected: "DRAFT, OPEN or LOCKED",
                detected: other.to_owned(),
            }),
        }
    }
}

/// The meeting state a mutation is judged against.
///
/// Always loaded from the database **inside** the mutating transaction. A
/// `Meeting` value obtained any other way is a snapshot of the past and must
/// not be used to decide whether a mutation may proceed (architecture rules
/// section 15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meeting {
    pub id: MeetingId,
    pub title: String,
    pub status: MeetingStatus,
}

impl Meeting {
    /// Refuse if the meeting no longer accepts mutations.
    ///
    /// The error names the meeting by title as well as id, because "this
    /// meeting is locked" is only actionable if the Host can tell which one
    /// (architecture rules section 22).
    pub fn ensure_mutable(&self) -> DomainResult<()> {
        if self.status.accepts_mutations() {
            Ok(())
        } else {
            Err(DomainError::MeetingLocked {
                meeting_id: self.id,
                title: self.title.clone(),
            })
        }
    }

    /// Validate a lifecycle transition against the *current* status.
    ///
    /// Only the two transitions PRD section 6 defines are legal, and each is
    /// legal from exactly one state. Re-opening an open meeting or re-locking a
    /// locked one is rejected rather than treated as a harmless no-op: a
    /// no-op would still write an audit record claiming something happened.
    pub fn ensure_transition(&self, to: MeetingStatus) -> DomainResult<()> {
        let expected = match to {
            MeetingStatus::Open => "DRAFT",
            MeetingStatus::Locked => "OPEN",
            MeetingStatus::Draft => {
                // Nothing returns to DRAFT, and there is no unlock.
                return Err(DomainError::InvalidTransition {
                    expected: "a forward transition to OPEN or LOCKED",
                    detected: self.status,
                });
            }
        };

        // The only two transitions PRD section 6 defines.
        let legal = matches!(
            (self.status, to),
            (MeetingStatus::Draft, MeetingStatus::Open)
                | (MeetingStatus::Open, MeetingStatus::Locked)
        );

        if legal {
            Ok(())
        } else {
            Err(DomainError::InvalidTransition {
                expected,
                detected: self.status,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(status: MeetingStatus) -> Meeting {
        Meeting {
            id: MeetingId::new(),
            title: "Weekly Coordination".to_owned(),
            status,
        }
    }

    #[test]
    fn status_strings_match_the_persisted_discriminators() {
        assert_eq!(MeetingStatus::Draft.as_str(), "DRAFT");
        assert_eq!(MeetingStatus::Open.as_str(), "OPEN");
        assert_eq!(MeetingStatus::Locked.as_str(), "LOCKED");

        for status in [
            MeetingStatus::Draft,
            MeetingStatus::Open,
            MeetingStatus::Locked,
        ] {
            assert_eq!(status.as_str().parse::<MeetingStatus>().unwrap(), status);
        }
    }

    #[test]
    fn an_unknown_status_string_is_rejected_with_what_was_expected() {
        let err = "EXPORTED".parse::<MeetingStatus>().unwrap_err();
        let message = err.to_string();
        assert!(message.contains("DRAFT, OPEN or LOCKED"), "{message}");
        assert!(message.contains("EXPORTED"), "{message}");
    }

    #[test]
    fn only_a_locked_meeting_refuses_mutations() {
        assert!(meeting(MeetingStatus::Draft).ensure_mutable().is_ok());
        assert!(meeting(MeetingStatus::Open).ensure_mutable().is_ok());

        let locked = meeting(MeetingStatus::Locked);
        let err = locked.ensure_mutable().unwrap_err();
        assert!(matches!(err, DomainError::MeetingLocked { .. }));
        // Actionable: the Host can tell which meeting refused.
        assert!(err.to_string().contains("Weekly Coordination"));
    }

    #[test]
    fn only_the_two_forward_transitions_are_legal() {
        assert!(meeting(MeetingStatus::Draft)
            .ensure_transition(MeetingStatus::Open)
            .is_ok());
        assert!(meeting(MeetingStatus::Open)
            .ensure_transition(MeetingStatus::Locked)
            .is_ok());
    }

    #[test]
    fn skipping_draft_to_locked_is_rejected() {
        let err = meeting(MeetingStatus::Draft)
            .ensure_transition(MeetingStatus::Locked)
            .unwrap_err();
        assert_eq!(
            err,
            DomainError::InvalidTransition {
                expected: "OPEN",
                detected: MeetingStatus::Draft,
            }
        );
    }

    #[test]
    fn repeating_a_transition_is_rejected_rather_than_a_silent_no_op() {
        // A no-op would still write an audit record saying the meeting was
        // locked, which would be a lie about what happened.
        assert!(meeting(MeetingStatus::Open)
            .ensure_transition(MeetingStatus::Open)
            .is_err());
        assert!(meeting(MeetingStatus::Locked)
            .ensure_transition(MeetingStatus::Locked)
            .is_err());
    }

    #[test]
    fn there_is_no_unlock_and_no_return_to_draft() {
        for status in [
            MeetingStatus::Draft,
            MeetingStatus::Open,
            MeetingStatus::Locked,
        ] {
            assert!(meeting(status)
                .ensure_transition(MeetingStatus::Draft)
                .is_err());
        }
        // Specifically: LOCKED never goes back to OPEN.
        assert!(meeting(MeetingStatus::Locked)
            .ensure_transition(MeetingStatus::Open)
            .is_err());
    }
}
