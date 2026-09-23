//! # app-remote
//!
//! Remote participation: self-contained HTML form generation, and the pieces of
//! the submission contract both sides of that exchange have to agree on.
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
//! ```text
//! generate:  Host context  ->  template + JSON island  ->  one HTML file
//! parse:     submission file  ->  SubmissionV1  ->  canonical form  ->  hash
//! ```
//!
//! # What this crate deliberately cannot do
//!
//! It holds **no database handle and no SQL**, and it **never constructs
//! [`app_core::Actor`]**. The crate that reads an untrusted file is not the
//! crate that produces authority: the actor is built in `src-tauri`, from the
//! Host-selected meeting and the database-resolved participant, after
//! validation and after the Host confirms (ADR-0021, decision 14). Boundary
//! guards assert both absences.
//!
//! # Scope
//!
//! Step 9 is **generation**. What is here is the generator, the submission shape
//! the offline form produces, and the canonical hash that decides whether two
//! submissions are the same artefact - the last because it is a byte-level
//! contract two languages must agree on, and agreement is proved by fixtures
//! rather than asserted.
//!
//! The **import pipeline is step 10** and is not here: no meeting or participant
//! resolution, no duplicate lookup, no lifecycle check, no `remote_submissions`
//! write, no transaction. [`RemoteError::MeetingMismatch`] is kept for it.
//!
//! Import remains version-aware when it arrives: a participant has exactly one
//! note, so an import updates that note and appends a `note_versions` row rather
//! than creating a second note (PRD 12.1, 13.1), and the whole of it runs in a
//! single transaction (section 11).

#![forbid(unsafe_code)]

mod canonical;
mod context;
mod generate;
mod submission;

pub use canonical::{canonical_form, content_hash, normalize_note, CANONICAL_PREFIX};
pub use context::{RemoteFormContext, SCHEMA_VERSION};
pub use generate::{
    generate, is_bundled, verify_artifact, CONTEXT_ELEMENT_ID, FORBIDDEN_IN_ARTIFACT,
};
pub use submission::{SubmissionV1, MAX_SUBMISSION_BYTES};

use thiserror::Error;

pub type RemoteResult<T> = Result<T, RemoteError>;

/// Rejection reasons for remote form generation and submission parsing.
///
/// Every variant is renderable as an actionable Host-facing message that names
/// both the expected and the detected value (architecture rules section 22).
/// None of them quotes an entire payload back: content can be long, and
/// repeating untrusted input into an error is a habit worth not having.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemoteError {
    /// The submission is not JSON, or not the shape a submission has.
    #[error("malformed submission: expected a remote submission object, detected {detail}")]
    Malformed { detail: String },

    /// The file is a submission, of a schema version this build does not know.
    #[error(
        "unsupported submission schema: expected version {expected}, detected version {detected}"
    )]
    UnsupportedSchema { expected: u32, detected: u64 },

    /// The artefact is larger than a submission is allowed to be.
    ///
    /// Checked before a file is read whole, so nothing reads an arbitrary number
    /// of bytes into memory, and checked again on what was read, so a file that
    /// grows mid-operation cannot get past the bound (ADR-0022 decision 16).
    #[error("submission too large: expected at most {limit} bytes, detected {detected} bytes")]
    TooLarge { limit: usize, detected: u64 },

    /// An identifier in the submission is not a canonical UUIDv7.
    ///
    /// Carries the field so the Host is told which one, and the detected text so
    /// they can see what arrived. It says nothing about whether the id names
    /// anything real - that is a lookup, and it belongs to import.
    #[error(
        "invalid {field} in submission: expected a canonical UUID version 7, detected {detected:?}"
    )]
    MalformedId {
        field: &'static str,
        detected: String,
    },

    /// The embedded remote form template is absent or has no injection point.
    ///
    /// A build problem on the Host's own machine, not a participant's doing.
    #[error("the remote form template cannot be used: {detail}")]
    Template { detail: String },

    /// The generated artefact failed the offline check it is generated to pass.
    ///
    /// Generation refuses rather than writing the file: an artefact that reaches
    /// a participant must already be the thing it claims to be.
    #[error("the generated remote form is not self-contained: {detail}")]
    Artifact { detail: String },

    /// The submission belongs to a different meeting than the selected one.
    ///
    /// Step 10 territory, kept here because the meeting a submission names is
    /// part of the contract this crate defines.
    #[error("submission belongs to another meeting")]
    MeetingMismatch,
}
