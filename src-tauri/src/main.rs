//! Tauri desktop shell (Host).
//!
//! This binary is a **transport and lifecycle host**, not a place for business
//! rules. Its jobs are:
//! - own the application state (database handle, LAN server lifecycle)
//! - expose Tauri commands to the Host UI, minting `app_core::Actor::Host`
//! - start/stop the LAN server as the meeting lifecycle requires
//!
//! Every mutation must be delegated to `app-core`.
//!
//! Status: skeleton. No commands are registered yet.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running the LAN Meeting Collaboration App");
}
