//! # app-core
//!
//! Domain core of the LAN Meeting Collaboration App.
//!
//! This crate is the **only** place where business rules live. Both transports
//! (`app-server` for LAN participants, `src-tauri` commands for the Host) must
//! route every mutation through this crate so that authorization, meeting lock
//! enforcement, auditing and note versioning cannot be bypassed.
//!
//! Responsibilities:
//! - actor resolution contract ([`actor::Actor`])
//! - authorization decisions
//! - meeting lock enforcement (inside the same transaction as the mutation)
//! - audit log policy
//! - note versioning policy
//!
//! Explicitly **not** responsible for: HTTP, WebSocket, SQL, file I/O, UI.
//!
//! Status: skeleton. No behaviour is implemented yet (Phase 1, step 2).

#![forbid(unsafe_code)]

pub mod actor;
pub mod error;

pub use actor::Actor;
pub use error::{CoreError, CoreResult};
