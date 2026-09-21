//! Tauri desktop shell (Host).
//!
//! This crate is a **transport and lifecycle host**, not a place for business
//! rules. Its jobs are:
//! - own the application state (the database handle and the mutation boundary)
//! - expose Tauri commands to the Host UI, minting `app_core::Actor::Host`
//! - start/stop the LAN server as the meeting lifecycle requires (not yet)
//!
//! Every mutation is delegated to `app-core`, and it could not be otherwise:
//! the port's write methods require an authorization proof only that crate can
//! mint (ADR-0012).
//!
//! # Layout
//!
//! | Module | Holds |
//! | --- | --- |
//! | [`dto`] | the IPC shapes, and the parsing that establishes types |
//! | [`error`] | `DomainError` -> the structured Host error (ADR-0015) |
//! | [`host`] | application state and the implementation of each command |
//! | [`commands`] | `#[tauri::command]` declarations, one line each |
//!
//! It is a library plus a thin binary so that [`host::HostState`] can be tested
//! against a real SQLite database without a running Tauri application.
//!
//! # Not here
//!
//! No LAN HTTP server, no WebSocket, no session or token resolution, no join
//! token, no QR code, no remote form or import, no export. This crate will host
//! the LAN server's *lifecycle* when that exists; it will not contain it.
//!
//! Status: Phase 1, step 4. Meeting and participant management, and the
//! read-only audit view, are exposed to the Host UI.

#![forbid(unsafe_code)]

pub mod commands;
pub mod dto;
pub mod error;
pub mod host;

pub use error::{ErrorCategory, HostError, HostErrorKind, HostResult};
pub use host::{HostState, DATABASE_FILE};

use tauri::Manager;

/// Build and run the Host application.
///
/// The database lives in the platform application-data directory, is created on
/// first run and migrated on every start. It is a local file and no port is
/// opened for it (PRD 22.7, 22.8).
///
/// A failure here is fatal by design: a Host window with no database would offer
/// actions that silently do nothing, which is worse than not starting.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let directory = app.path().app_data_dir()?;
            std::fs::create_dir_all(&directory)?;
            let state = HostState::open(directory.join(DATABASE_FILE))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_meeting,
            commands::update_meeting,
            commands::open_meeting,
            commands::list_meetings,
            commands::get_meeting,
            commands::add_participant,
            commands::update_participant,
            commands::remove_participant,
            commands::list_participants,
            commands::list_audit_entries,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the LAN Meeting Collaboration App");
}
