//! # app-remote
//!
//! Remote participation: self-contained HTML form generation, and the import
//! pipeline for the submission files participants send back.
//!
//! Two hard rules govern this crate:
//!
//! 1. **The generated form is self-contained.** No CDN, no external script or
//!    stylesheet, no network call. It must open from a `file://` URL with the
//!    machine fully offline (architecture rules sections 6 and 20).
//! 2. **Submission files are untrusted input.** The application generated the
//!    form, but the returned file is external data: never trust the filename,
//!    never treat a name or id carried in the payload as authority, and never
//!    INSERT before the full validation pipeline has passed (section 10).
//!
//! Import is version-aware: a participant has exactly one note, so an import
//! updates that note and appends a `note_versions` row rather than creating a
//! second note (PRD 12.1, 13.1).
//!
//! The whole import runs in a single transaction - a partial import is not
//! permitted (section 11).
//!
//! Status: skeleton. No generation, parsing or import exists yet
//! (Phase 1, steps 7-8).

#![forbid(unsafe_code)]

use thiserror::Error;

pub type RemoteResult<T> = Result<T, RemoteError>;

/// Rejection reasons for a remote submission.
///
/// Each variant must be renderable as an actionable Host-facing message that
/// names both the expected and the detected value (architecture rules
/// section 22).
#[derive(Debug, Error)]
pub enum RemoteError {
    /// The file could not be parsed as a supported submission schema version.
    #[error("unsupported or malformed submission schema")]
    Schema,

    /// The submission belongs to a different meeting than the selected one.
    #[error("submission belongs to another meeting")]
    MeetingMismatch,
}
