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
//! | [`lan`] | the LAN server's start/stop lifecycle |
//! | [`qr`] | QR matrices for the join URL, generated locally |
//! | [`commands`] | `#[tauri::command]` declarations, one line each |
//!
//! It is a library plus a thin binary so that [`host::HostState`] can be tested
//! against a real SQLite database without a running Tauri application.
//!
//! # The LAN server
//!
//! This crate owns the server's *lifecycle* and not the server: `app-server`
//! holds the routes, the extractors and the participant contract. What lives
//! here is the decision to start and stop it, and the QR for its join URL.
//!
//! Both transports share **one** `Domain` over one database, so a rule cannot
//! be enforced differently depending on who is asking (ADR-0012).
//!
//! # Not here
//!
//! No WebSocket - realtime is its own roadmap step. No remote form or import,
//! no notes, no export.
//!
//! Status: Phase 1, step 6. Meeting and participant management, the read-only
//! audit view, and the LAN server with the join and identity-claim flow.

#![forbid(unsafe_code)]

pub mod commands;
pub mod dto;
pub mod error;
pub mod host;
pub mod lan;
pub mod qr;

pub use error::{ErrorCategory, HostError, HostErrorKind, HostResult};
pub use host::{HostState, DATABASE_FILE};
pub use lan::{LanLifecycle, LanServerStatus};
pub use qr::QrMatrix;

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

            // The LAN server shares this database and this mutation boundary.
            // It is not started here: binding a LAN-reachable socket is the
            // Host's decision, not a side effect of launching (ADR-0016).
            app.manage(LanLifecycle::new(state.shared_db(), state.shared_domain()));
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
            commands::start_lan_server,
            commands::stop_lan_server,
            commands::lan_server_status,
            commands::list_lan_interfaces,
            commands::issue_join_token,
            commands::approve_participant_claim,
            commands::revoke_participant_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the LAN Meeting Collaboration App");
}
