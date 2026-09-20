//! # app-export
//!
//! Deterministic export renderers.
//!
//! Phase 1 scope: **Markdown** and **TXT / AI Context**.
//! PDF export is intentionally deferred to Phase 2 pending a technical spike -
//! see `docs/adr/0004-defer-pdf-export.md`. No PDF dependency may be added
//! during Phase 1.
//!
//! Rules:
//! - the same input must always produce the same output; every query feeding an
//!   export needs an explicit total ordering (architecture rules 19, 26.4)
//! - exports state the meeting timezone explicitly (26.3)
//! - AI Context only formats and structures. It must not summarize, infer,
//!   interpret, or invent. No AI API is involved anywhere in this crate.
//!
//! Status: skeleton. No renderers exist yet (Phase 1, step 10).

#![forbid(unsafe_code)]

use thiserror::Error;

pub type ExportResult<T> = Result<T, ExportError>;

/// Errors produced while rendering an export.
#[derive(Debug, Error)]
pub enum ExportError {
    /// Placeholder until renderers are implemented.
    #[error("export failed: {0}")]
    Other(String),
}
