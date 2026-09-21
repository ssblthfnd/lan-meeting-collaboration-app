//! Binary entry point for the Host desktop application.
//!
//! Everything lives in the library beside this file, so that the command layer
//! can be tested without a running Tauri application. See `lib.rs`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

fn main() {
    lan_meeting_app_lib::run();
}
