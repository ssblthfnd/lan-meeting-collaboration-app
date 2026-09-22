//! The Tauri command surface.
//!
//! Every function here is a one-line delegation to [`HostState`]. That is
//! deliberate: `#[tauri::command]` functions need a live Tauri application to
//! call, so keeping them empty means the logic they would otherwise hold lives
//! in `HostState` instead, where a test can reach it against a real database.
//! What cannot be tested tends to be where a rule quietly appears.
//!
//! `rename_all = "snake_case"` on every command, so an argument is named the
//! same in TypeScript, in Rust and in SQLite. Tauri's default would translate
//! `meetingId` to `meeting_id`, which works but adds a casing convention that
//! only exists at this boundary and only shows up when it is got wrong.

use tauri::State;

use crate::dto::{
    AuditEntryDto, JoinTokenIssuedDto, LanInterfaceDto, MeetingConfigurationInput,
    MeetingCreatedDto, MeetingDetailDto, MeetingSummaryDto, MeetingTransitionedDto,
    MeetingUpdatedDto, ParticipantAddedDto, ParticipantDetailsInput, ParticipantPresenceDto,
    ParticipantRemovedDto, ParticipantSummaryDto, ParticipantUpdatedDto, SessionChangedDto,
};
use crate::error::{HostError, HostErrorKind, HostResult};
use crate::host::HostState;
use crate::lan::{LanLifecycle, LanServerStatus};

/* -------------------------------------------------------------------------
 * Meetings
 * ------------------------------------------------------------------------- */

#[tauri::command(rename_all = "snake_case")]
pub fn create_meeting(
    state: State<'_, HostState>,
    configuration: MeetingConfigurationInput,
) -> HostResult<MeetingCreatedDto> {
    state.create_meeting(configuration)
}

#[tauri::command(rename_all = "snake_case")]
pub fn update_meeting(
    state: State<'_, HostState>,
    meeting_id: String,
    configuration: MeetingConfigurationInput,
) -> HostResult<MeetingUpdatedDto> {
    state.update_meeting(&meeting_id, configuration)
}

#[tauri::command(rename_all = "snake_case")]
pub fn open_meeting(
    state: State<'_, HostState>,
    meeting_id: String,
) -> HostResult<MeetingTransitionedDto> {
    state.open_meeting(&meeting_id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_meetings(state: State<'_, HostState>) -> HostResult<Vec<MeetingSummaryDto>> {
    state.list_meetings()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_meeting(
    state: State<'_, HostState>,
    meeting_id: String,
) -> HostResult<MeetingDetailDto> {
    state.get_meeting(&meeting_id)
}

/* -------------------------------------------------------------------------
 * Participants
 * ------------------------------------------------------------------------- */

#[tauri::command(rename_all = "snake_case")]
pub fn add_participant(
    state: State<'_, HostState>,
    meeting_id: String,
    details: ParticipantDetailsInput,
) -> HostResult<ParticipantAddedDto> {
    state.add_participant(&meeting_id, details)
}

#[tauri::command(rename_all = "snake_case")]
pub fn update_participant(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
    details: ParticipantDetailsInput,
) -> HostResult<ParticipantUpdatedDto> {
    state.update_participant(&meeting_id, &participant_id, details)
}

#[tauri::command(rename_all = "snake_case")]
pub fn remove_participant(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
) -> HostResult<ParticipantRemovedDto> {
    state.remove_participant(&meeting_id, &participant_id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_participants(
    state: State<'_, HostState>,
    meeting_id: String,
) -> HostResult<Vec<ParticipantSummaryDto>> {
    state.list_participants(&meeting_id)
}

/* -------------------------------------------------------------------------
 * LAN access
 *
 * The server is started and stopped by the Host, never as a side effect of
 * opening a meeting (ADR-0016). Which meetings are joinable is decided per
 * request, by re-reading the meeting's status.
 * ------------------------------------------------------------------------- */

#[tauri::command(rename_all = "snake_case")]
pub async fn start_lan_server(
    state: State<'_, HostState>,
    lan: State<'_, LanLifecycle>,
    port: Option<u16>,
) -> HostResult<LanServerStatus> {
    // Touching `state` keeps the database alive for the server's lifetime and
    // makes the sharing explicit at the call site.
    let _ = state.shared_db();
    lan.start(port).await.map_err(|error| {
        eprintln!("[host] {error}");
        HostError::new(
            HostErrorKind::Persistence,
            // The bind error already names the port and the two likely causes,
            // so the Host is told what to do rather than that it failed.
            error.to_string(),
        )
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stop_lan_server(lan: State<'_, LanLifecycle>) -> HostResult<LanServerStatus> {
    Ok(lan.stop().await)
}

#[tauri::command(rename_all = "snake_case")]
pub fn lan_server_status(lan: State<'_, LanLifecycle>) -> HostResult<LanServerStatus> {
    Ok(lan.status())
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_lan_interfaces(state: State<'_, HostState>) -> HostResult<Vec<LanInterfaceDto>> {
    Ok(state.lan_interfaces())
}

#[tauri::command(rename_all = "snake_case")]
pub fn issue_join_token(
    state: State<'_, HostState>,
    meeting_id: String,
    address: String,
    port: u16,
) -> HostResult<JoinTokenIssuedDto> {
    state.issue_join_token(&meeting_id, &address, port)
}

#[tauri::command(rename_all = "snake_case")]
pub fn approve_participant_claim(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
) -> HostResult<SessionChangedDto> {
    state.approve_participant_claim(&meeting_id, &participant_id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn revoke_participant_session(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
) -> HostResult<SessionChangedDto> {
    state.revoke_participant_session(&meeting_id, &participant_id)
}

/* -------------------------------------------------------------------------
 * Presence
 *
 * A read. The live half comes from the LAN server's connection registry and
 * the durable half from SQLite; neither is a mutation, and neither is audited
 * (ADR-0018). The Host UI seeds its roster with this and then follows the
 * `presence.changed` domain events.
 * ------------------------------------------------------------------------- */

#[tauri::command(rename_all = "snake_case")]
pub fn list_participant_presence(
    state: State<'_, HostState>,
    lan: State<'_, LanLifecycle>,
    meeting_id: String,
) -> HostResult<Vec<ParticipantPresenceDto>> {
    state.list_participant_presence(&meeting_id, |id| lan.connected(id))
}

/* -------------------------------------------------------------------------
 * Audit
 *
 * Read-only. There is no command to write, edit or delete an entry, and there
 * must never be one: the log is append-only (architecture rules section 17) and
 * entries are written only as part of a domain mutation.
 * ------------------------------------------------------------------------- */

#[tauri::command(rename_all = "snake_case")]
pub fn list_audit_entries(
    state: State<'_, HostState>,
    meeting_id: String,
) -> HostResult<Vec<AuditEntryDto>> {
    state.list_audit_entries(&meeting_id)
}
