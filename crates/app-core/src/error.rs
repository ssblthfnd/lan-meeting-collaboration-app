//! Domain error type.
//!
//! Errors must be actionable (architecture rules section 22): they carry enough
//! context for the UI to explain *what* was rejected and *why*.

use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

/// Errors produced by the domain core.
///
/// Variants are added as the corresponding rules are implemented.
#[derive(Debug, Error)]
pub enum CoreError {
    /// The actor is not allowed to perform this action on this target.
    #[error("not authorized: {action} on {target}")]
    NotAuthorized { action: String, target: String },

    /// The meeting is LOCKED; no mutation is permitted.
    #[error("meeting {meeting_id} is locked and cannot be modified")]
    MeetingLocked { meeting_id: String },

    /// Input failed domain validation.
    #[error("invalid input: {0}")]
    Invalid(String),
}
