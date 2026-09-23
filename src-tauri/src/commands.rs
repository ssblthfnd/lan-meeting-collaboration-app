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

use tauri::Manager;
use tauri::State;

use crate::dto::{
    AuditEntryDto, JoinTokenIssuedDto, LanInterfaceDto, MeetingConfigurationInput,
    MeetingCreatedDto, MeetingDetailDto, MeetingSummaryDto, MeetingTransitionedDto,
    MeetingUpdatedDto, NoteDetailDto, NoteOverviewDto, NoteVersionDetailDto, NoteVersionSummaryDto,
    NoteWrittenDto, ParticipantAddedDto, ParticipantDetailsInput, ParticipantPresenceDto,
    ParticipantRemovedDto, ParticipantSummaryDto, ParticipantUpdatedDto, PendingSubmissionDto,
    RemoteFormGeneratedDto, RemoteSubmissionImportedDto, RemoteSubmissionPreviewDto,
    SessionChangedDto,
};
use crate::error::{HostError, HostErrorKind, HostResult};
use crate::host::{HostState, REMOTE_FORM_DIRECTORY};
use crate::lan::{LanLifecycle, LanServerStatus};
use crate::remote_import::PendingImport;

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
 * Notes
 *
 * The Host may read and write any participant's note while the meeting is not
 * LOCKED (PRD section 15). The write goes through `Domain::write_note`, which
 * decides the version, appends the immutable history row and writes the audit
 * record inside one transaction, then publishes `note.changed`.
 *
 * There is deliberately no restore command. Version history is view-only in
 * step 8 (ADR-0019), and a command that wrote a historical body back would be
 * the whole of restore arriving through the side door.
 * ------------------------------------------------------------------------- */

#[tauri::command(rename_all = "snake_case")]
pub fn get_participant_note(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
) -> HostResult<Option<NoteDetailDto>> {
    state.get_participant_note(&meeting_id, &participant_id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn write_participant_note(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
    content: String,
) -> HostResult<NoteWrittenDto> {
    state.write_participant_note(&meeting_id, &participant_id, &content)
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_note_versions(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
) -> HostResult<Vec<NoteVersionSummaryDto>> {
    state.list_note_versions(&meeting_id, &participant_id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_note_version(
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
    version: i64,
) -> HostResult<Option<NoteVersionDetailDto>> {
    state.get_note_version(&meeting_id, &participant_id, version)
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_notes_overview(
    state: State<'_, HostState>,
    meeting_id: String,
) -> HostResult<Vec<NoteOverviewDto>> {
    state.list_notes_overview(&meeting_id)
}

/* -------------------------------------------------------------------------
 * Remote participation
 *
 * Generation only. A form is produced for a participant the Host selected, the
 * file is written by Rust, and the path comes back. Importing a submission is
 * step 10 and there is deliberately no command for it yet.
 * ------------------------------------------------------------------------- */

/// Write a standalone offline remote form for one participant.
///
/// The one command that touches the filesystem, and the only one that needs
/// `AppHandle`: the output directory is Tauri's application-data directory,
/// resolved here. **The window never names a path.** It names a meeting and a
/// participant; a renderer that could choose a directory could write anywhere,
/// and no filesystem or dialog plugin is installed to let it try
/// (ADR-0021 decision 10).
#[tauri::command(rename_all = "snake_case")]
pub fn generate_remote_form(
    app: tauri::AppHandle,
    state: State<'_, HostState>,
    meeting_id: String,
    participant_id: String,
) -> HostResult<RemoteFormGeneratedDto> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| {
            eprintln!("[host] resolving the application data directory: {error}");
            HostError::new(
                HostErrorKind::Persistence,
                "This installation has no application data folder to write into.".to_owned(),
            )
        })?
        .join(REMOTE_FORM_DIRECTORY);

    state.generate_remote_form(&meeting_id, &participant_id, &directory)
}

/* -------------------------------------------------------------------------
 * Remote submission import
 *
 * Four commands, and none of them takes an artefact or a path. The bytes live
 * in `PendingImport` - put there by Rust when a file is dropped, or by
 * `remote_submission_from_text` when the Host pastes - and both preview and
 * confirmation read them from there.
 *
 * That is the trust boundary, expressed as a signature: a window that cannot
 * name a file cannot read an arbitrary one, and a window that cannot hand back
 * an artefact cannot confirm one the Host never saw (ADR-0022 decision 4).
 * ------------------------------------------------------------------------- */

/// Take pasted submission text as the pending artefact.
///
/// Text is data rather than a filesystem path, so it may come from the window.
/// It is bounded by the same 256 KiB limit a dropped file is, and it is stored
/// in Rust exactly as a dropped file would be - confirmation reads it from
/// there, never from a second call.
#[tauri::command(rename_all = "snake_case")]
pub fn remote_submission_from_text(
    pending: State<'_, PendingImport>,
    text: String,
) -> HostResult<PendingSubmissionDto> {
    let artifact = pending.accept_text(text)?;
    Ok(PendingSubmissionDto {
        bytes: artifact.raw.len(),
        origin: artifact.origin,
    })
}

/// Everything the Host should see before deciding whether to import.
///
/// Read-only. Nothing it reports is authority: the import transaction asks the
/// database again and is what decides.
#[tauri::command(rename_all = "snake_case")]
pub fn preview_remote_submission(
    state: State<'_, HostState>,
    pending: State<'_, PendingImport>,
    meeting_id: String,
) -> HostResult<RemoteSubmissionPreviewDto> {
    let artifact = pending.require()?;
    state.preview_remote_submission(&meeting_id, &artifact.raw, &artifact.origin)
}

/// Import the pending submission into the Host-selected meeting.
///
/// Takes no artefact: it uses the exact bytes that were previewed. Every
/// security-critical check runs again, inside the domain's own transaction.
#[tauri::command(rename_all = "snake_case")]
pub fn confirm_remote_submission(
    state: State<'_, HostState>,
    pending: State<'_, PendingImport>,
    meeting_id: String,
) -> HostResult<RemoteSubmissionImportedDto> {
    let artifact = pending.require()?;
    let imported = state.import_remote_submission(&meeting_id, &artifact.raw)?;

    // Only after the domain committed. A refusal leaves the artefact pending so
    // the Host can look at it again.
    pending.clear();
    Ok(imported)
}

/// Forget whatever submission was waiting.
#[tauri::command(rename_all = "snake_case")]
pub fn clear_remote_submission(pending: State<'_, PendingImport>) -> HostResult<()> {
    pending.clear();
    Ok(())
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
