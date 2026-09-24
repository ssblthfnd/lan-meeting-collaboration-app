//! # app-export
//!
//! Deterministic export renderers.
//!
//! Phase 1 scope: **Markdown** and **TXT / AI Context** ([`document`]).
//! PDF export is intentionally deferred to Phase 2 pending a technical spike -
//! see `docs/adr/0004-defer-pdf-export.md`. No PDF dependency may be added
//! during Phase 1.
//!
//! # A pure rendering layer
//!
//! This crate has no filesystem access, no SQLite access, no Tauri API, no
//! authorization decision and no UI concern. Every renderer is a pure
//! function from an [`document::ExportDocument`] - plain data the caller
//! (`src-tauri`) assembles from `HostQueries` - to a `String`. Rendering
//! never fails: the input has already passed `app_core::note::validate` by
//! the time it reaches [`markdown_ast::parse_markdown`], so there is nothing
//! for a renderer to refuse.
//!
//! Rules:
//! - the same input must always produce the same output; every collection a
//!   renderer walks is already explicitly ordered by the caller (architecture
//!   rules 19, 26.4)
//! - exports state the meeting timezone explicitly (26.3)
//! - AI Context only formats and structures. It must not summarize, infer,
//!   interpret, or invent, and performs no heading recognition of any kind -
//!   see [`document::render_ai_context`]. No AI API is involved anywhere in
//!   this crate.
//! - [`markdown_ast`] is a bounded parser for the canonical GFM subset
//!   already allowlisted by `app_core::note` - not a general Markdown/
//!   CommonMark parser and not a new dialect.

#![forbid(unsafe_code)]

pub mod document;
pub mod markdown_ast;
pub mod plain_text;

pub use document::{
    render_ai_context, render_markdown, render_txt, ExportDocument, ExportMeeting, ExportNote,
    ExportParticipant, ExportTimeline,
};
