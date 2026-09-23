//! Tauri desktop shell (Host).
//!
//! This crate is a **transport and lifecycle host**, not a place for business
//! rules. Its jobs are:
//! - own the application state (the database handle and the mutation boundary)
//! - expose Tauri commands to the Host UI, minting `app_core::Actor::Host`
//! - start and stop the LAN server when the Host asks
//! - assemble the event sink that reaches both audiences, and emit the Host's
//!   half of it into the window over IPC
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
//! | [`events`] | domain events -> the Host window, over Tauri IPC |
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
//! # Realtime, and why the Host has no socket
//!
//! One `EventSink` is built here and handed to the `Domain`. It is a composite:
//!
//! ```text
//! Domain
//!  └── CompositeSink
//!       ├── Realtime      -> participant WebSockets, when the server runs
//!       └── TauriEventSink -> this window, always
//! ```
//!
//! The Host is in the same process as the database and hears events over IPC.
//! Giving it a WebSocket to `127.0.0.1` would put the Host's own view on the
//! transport that faces untrusted input, and would stop working the moment the
//! Host chose not to start that server (ADR-0018).
//!
//! # Not here
//!
//! No remote form or import, no participant note editing, no note restore, no
//! meeting lock command, no export.
//!
//! Status: Phase 1, step 8. Meeting and participant management, the read-only
//! audit view, the LAN server with the join and identity-claim flow, realtime
//! notification with presence, and Host note editing with view-only version
//! history.

#![forbid(unsafe_code)]

pub mod commands;
pub mod dto;
pub mod error;
pub mod events;
pub mod host;
pub mod lan;
pub mod qr;
pub mod remote_import;

pub use error::{ErrorCategory, HostError, HostErrorKind, HostResult};
pub use events::{TauriEventSink, DOMAIN_EVENT};
pub use host::{HostState, DATABASE_FILE, REMOTE_FORM_DIRECTORY};
pub use lan::{LanLifecycle, LanServerStatus};
pub use qr::QrMatrix;
pub use remote_import::{PendingImport, REMOTE_SUBMISSION_PENDING};

use std::sync::Arc;

use app_core::event::{CompositeSink, EventSink};
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

            // Built before the database, because the mutation boundary is
            // constructed with it: a `Domain` publishes to the sink it was
            // given, and there is no way to attach one afterwards.
            let realtime = Arc::new(app_server::Realtime::new());
            let events: Arc<dyn EventSink> = Arc::new(CompositeSink::new(vec![
                Arc::clone(&realtime) as Arc<dyn EventSink>,
                Arc::new(TauriEventSink::new(app.handle().clone())),
            ]));

            let state =
                HostState::open_with_events(directory.join(DATABASE_FILE), Arc::clone(&events))?;

            // The LAN server shares this database, this mutation boundary and
            // this channel. It is not started here: binding a LAN-reachable
            // socket is the Host's decision, not a side effect of launching
            // (ADR-0016).
            app.manage(LanLifecycle::new(
                state.shared_db(),
                state.shared_domain(),
                events,
                realtime,
            ));
            app.manage(state);

            // The one pending submission slot. A dropped file's path arrives
            // here, in Rust, and never reaches the window: what the renderer
            // gets is a notification that something is waiting (ADR-0022 d17).
            app.manage(PendingImport::new());

            Ok(())
        })
        // A file dropped on the Host window. The path is handled in Rust and
        // is never sent to the renderer; one line here, because `run()` cannot
        // be tested and everything decidable about a drop is decided in
        // `remote_import` (ADR-0022 decision 17).
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                remote_import::announce_drop(window.app_handle(), paths);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_meeting,
            commands::update_meeting,
            commands::open_meeting,
            commands::lock_meeting,
            commands::list_meetings,
            commands::get_meeting,
            commands::add_participant,
            commands::update_participant,
            commands::remove_participant,
            commands::list_participants,
            commands::list_audit_entries,
            commands::list_participant_presence,
            commands::get_participant_note,
            commands::write_participant_note,
            commands::list_note_versions,
            commands::get_note_version,
            commands::list_notes_overview,
            commands::generate_remote_form,
            commands::remote_submission_from_text,
            commands::preview_remote_submission,
            commands::confirm_remote_submission,
            commands::clear_remote_submission,
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
