//! # app-server
//!
//! Local HTTP + WebSocket server for LAN participants.
//!
//! This crate is a **transport**, not a place for business rules. It:
//! - binds a LAN-reachable address chosen by the Host (it never assumes
//!   `127.0.0.1` is reachable by participants - architecture rules section 4)
//! - serves the embedded `apps/lan-ui` bundle and the `/join/{token}` entry
//! - resolves an `app_core::Actor::Participant` from a stored session token
//!   hash, and never from a client-supplied participant id (section 14.1)
//! - delegates every mutation to `app-core`
//! - broadcasts WebSocket events only *after* a successful SQLite commit, on
//!   audience-scoped channels so a participant can never receive another
//!   participant note (PRD 5.2, architecture rules section 16)
//!
//! WebSocket is a notification channel, never a source of truth.
//!
//! Status: skeleton. No routes, sessions or sockets exist yet
//! (Phase 1, steps 4-5).

#![forbid(unsafe_code)]

use thiserror::Error;

pub type ServerResult<T> = Result<T, ServerError>;

/// Errors produced by the LAN transport.
#[derive(Debug, Error)]
pub enum ServerError {
    /// The configured address/port could not be bound.
    ///
    /// This must surface in the Host UI with an actionable message
    /// (architecture rules section 22), including the likely causes: the port
    /// is already in use, or the Windows Firewall / network profile is
    /// blocking inbound connections.
    #[error("failed to bind {addr}: {reason}")]
    Bind { addr: String, reason: String },
}
