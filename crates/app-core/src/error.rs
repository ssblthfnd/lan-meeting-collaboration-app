//! Domain error model.
//!
//! Errors must be actionable (architecture rules section 22): they name the
//! expected and the detected value wherever that information exists, so a
//! transport can render something better than "something went wrong".
//!
//! These variants are the **public contract** for every future transport. They
//! deliberately carry no SQLite types, no HTTP status codes and no transport
//! vocabulary: a persistence failure arrives as [`DomainError::Persistence`]
//! with an opaque diagnostic string, not as a `rusqlite::Error`. Mapping a
//! domain error onto an HTTP status or a Tauri error payload is the
//! transport's job, and is deliberately not decided here.

use thiserror::Error;

use crate::id::{MeetingId, ParticipantId};
use crate::meeting::MeetingStatus;

pub type DomainResult<T> = Result<T, DomainError>;

/// Everything the domain layer can refuse or fail to do.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    /// The actor has no standing in this meeting at all.
    ///
    /// Distinct from [`DomainError::Forbidden`]: this is "you are not a party
    /// to this meeting", not "you are, but you may not do that". A participant
    /// acting on a meeting other than the one their session is bound to lands
    /// here (architecture rules section 14.1).
    #[error("actor is not a party to meeting {meeting_id}")]
    Unauthorized { meeting_id: MeetingId },

    /// The actor is a party to the meeting, but the operation is not permitted
    /// for their role.
    #[error("{actor_type} may not {action} on {target}")]
    Forbidden {
        actor_type: &'static str,
        action: &'static str,
        target: &'static str,
    },

    /// No such meeting.
    #[error("meeting not found: {meeting_id}")]
    MeetingNotFound { meeting_id: MeetingId },

    /// The meeting is LOCKED and no longer accepts mutations (PRD section 19).
    #[error("meeting {title:?} ({meeting_id}) is locked; no changes are permitted")]
    MeetingLocked {
        meeting_id: MeetingId,
        title: String,
    },

    /// The meeting has moved past `DRAFT`, so its configuration and roster are
    /// settled (ADR-0013).
    ///
    /// Distinct from [`DomainError::MeetingLocked`], which is the stronger
    /// statement that *nothing* may change. A meeting that is `OPEN` still
    /// accepts notes; it just no longer accepts changes to what the meeting is
    /// or who is in it. Distinct from [`DomainError::Forbidden`] too: no actor
    /// may do this, so it is not a question of the actor's role.
    #[error(
        "meeting {meeting_id} is no longer being prepared: \
             expected status DRAFT, detected {detected}"
    )]
    MeetingNotDraft {
        meeting_id: MeetingId,
        detected: MeetingStatus,
    },

    /// No such participant in this meeting.
    ///
    /// Also returned when the participant exists but belongs to a different
    /// meeting: from the caller's point of view those are the same fact, and
    /// distinguishing them would leak the existence of other meetings' rosters.
    #[error("participant {participant_id} is not part of meeting {meeting_id}")]
    ParticipantNotFound {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    },

    /// The requested lifecycle transition is not one the PRD defines.
    #[error("invalid meeting state transition: expected {expected}, detected {detected}")]
    InvalidTransition {
        expected: &'static str,
        detected: MeetingStatus,
    },

    /// Input failed domain validation.
    #[error("invalid {field}: expected {expected}, detected {detected}")]
    Validation {
        field: &'static str,
        expected: &'static str,
        detected: String,
    },

    /// A concurrent mutation won a race this one cannot recover from.
    ///
    /// The note version sequence is the case that matters: the database has a
    /// `UNIQUE(note_id, version)` constraint, so two writers computing the same
    /// next version cannot both commit (ADR-0011).
    #[error("the operation conflicted with a concurrent change: {detail}")]
    Conflict { detail: String },

    /// The database failed for a reason the domain cannot interpret.
    ///
    /// `detail` is diagnostic text for logs and bug reports. It is not part of
    /// the contract and must not be parsed.
    #[error("persistence failure while {operation}: {detail}")]
    Persistence {
        operation: &'static str,
        detail: String,
    },
}

impl DomainError {
    /// Whether this error denotes a refusal rather than a failure.
    ///
    /// A refusal is an expected answer that a transport should render to the
    /// user; a failure is a bug or an operational problem.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        !matches!(self, DomainError::Persistence { .. })
    }
}
