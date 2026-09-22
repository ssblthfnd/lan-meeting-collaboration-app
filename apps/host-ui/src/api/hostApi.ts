/**
 * The only module that talks to Tauri.
 *
 * Every command the backend registers appears here exactly once, wrapped in a
 * typed function. Components call these functions and never `invoke` directly:
 * a command name is a string, and a string repeated across twenty components is
 * a typo waiting to become a runtime failure in one of them.
 *
 * Nothing here decides anything. There is no validation, no permission check and
 * no meeting-status logic - all of that is `app-core`'s, enforced inside a
 * database transaction (ADR-0012, ADR-0013). This file translates a call into an
 * IPC message and a rejection into a typed {@link HostError}.
 *
 * Argument keys are `snake_case`, matching the Rust parameters and the SQLite
 * columns, so no casing is translated anywhere along the boundary.
 *
 * # Events come through here too
 *
 * {@link onDomainEvent} is the other half of the boundary: the backend
 * announces every committed mutation over Tauri IPC, and this module is the
 * only one that subscribes, for the same reason it is the only one that calls
 * `invoke`. The Host has no WebSocket - it is in the same process as the
 * database, so a socket to itself would route its own view through the
 * transport that exists to face untrusted input (ADR-0018).
 *
 * An event is a cue to refetch, never data to render. The payload carries
 * identifiers, a version and a timestamp; the value lives behind a command.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { HOST_DOMAIN_EVENT } from '@lan-meeting/contracts';
import type {
  AuditEntry,
  HostDomainEvent,
  HostError,
  JoinTokenIssued,
  LanInterface,
  LanServerStatus,
  MeetingConfigurationInput,
  MeetingCreated,
  MeetingDetail,
  MeetingId,
  MeetingSummary,
  MeetingTransitioned,
  MeetingUpdated,
  NoteDetail,
  NoteOverview,
  NoteVersionDetail,
  NoteVersionSummary,
  NoteWritten,
  ParticipantAdded,
  ParticipantDetailsInput,
  ParticipantId,
  ParticipantPresence,
  ParticipantRemoved,
  ParticipantSummary,
  ParticipantUpdated,
  SessionChanged,
} from '@lan-meeting/contracts';

import { toHostError } from './errors';

/**
 * Send one command, normalising a rejection into a {@link HostError}.
 *
 * Tauri rejects with whatever the command's error type serialized to, which for
 * every command here is a `HostError`. It can also reject with a string if the
 * IPC layer itself fails - before a command ran, or because the command name is
 * unknown - so the rejection is normalised rather than trusted.
 */
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (rejection) {
    throw toHostError(rejection, command);
  }
}

/* -------------------------------------------------------------------------
 * Meetings
 * ------------------------------------------------------------------------- */

/** Every meeting, most recent schedule first. */
export function listMeetings(): Promise<readonly MeetingSummary[]> {
  return call<MeetingSummary[]>('list_meetings');
}

/** One meeting in full. Rejects with `meeting_not_found` if it is gone. */
export function getMeeting(meetingId: MeetingId): Promise<MeetingDetail> {
  return call<MeetingDetail>('get_meeting', { meeting_id: meetingId });
}

/** Create a meeting. It always starts in `DRAFT`. */
export function createMeeting(
  configuration: MeetingConfigurationInput,
): Promise<MeetingCreated> {
  return call<MeetingCreated>('create_meeting', { configuration });
}

/** Replace a `DRAFT` meeting's configuration. */
export function updateMeeting(
  meetingId: MeetingId,
  configuration: MeetingConfigurationInput,
): Promise<MeetingUpdated> {
  return call<MeetingUpdated>('update_meeting', {
    meeting_id: meetingId,
    configuration,
  });
}

/** Move a meeting from `DRAFT` to `OPEN`. */
export function openMeeting(meetingId: MeetingId): Promise<MeetingTransitioned> {
  return call<MeetingTransitioned>('open_meeting', { meeting_id: meetingId });
}

/* -------------------------------------------------------------------------
 * Participants
 * ------------------------------------------------------------------------- */

/** A meeting's roster, ordered by name. */
export function listParticipants(
  meetingId: MeetingId,
): Promise<readonly ParticipantSummary[]> {
  return call<ParticipantSummary[]>('list_participants', { meeting_id: meetingId });
}

/** Add a participant to a `DRAFT` meeting's roster. */
export function addParticipant(
  meetingId: MeetingId,
  details: ParticipantDetailsInput,
): Promise<ParticipantAdded> {
  return call<ParticipantAdded>('add_participant', {
    meeting_id: meetingId,
    details,
  });
}

/** Replace a participant's details. */
export function updateParticipant(
  meetingId: MeetingId,
  participantId: ParticipantId,
  details: ParticipantDetailsInput,
): Promise<ParticipantUpdated> {
  return call<ParticipantUpdated>('update_participant', {
    meeting_id: meetingId,
    participant_id: participantId,
    details,
  });
}

/** Remove a participant from a `DRAFT` meeting's roster. */
export function removeParticipant(
  meetingId: MeetingId,
  participantId: ParticipantId,
): Promise<ParticipantRemoved> {
  return call<ParticipantRemoved>('remove_participant', {
    meeting_id: meetingId,
    participant_id: participantId,
  });
}

/* -------------------------------------------------------------------------
 * LAN access
 *
 * The server is started and stopped by the Host. Opening a meeting does not
 * start it: binding a LAN-reachable socket is a decision, not a side effect
 * (ADR-0016).
 * ------------------------------------------------------------------------- */

/** Whether the LAN server is running, and on which port. */
export function lanServerStatus(): Promise<LanServerStatus> {
  return call<LanServerStatus>('lan_server_status');
}

/** Start serving participants. `null` port means the default. */
export function startLanServer(port: number | null): Promise<LanServerStatus> {
  return call<LanServerStatus>('start_lan_server', { port });
}

/** Stop serving. Waits until the port is actually free. */
export function stopLanServer(): Promise<LanServerStatus> {
  return call<LanServerStatus>('stop_lan_server');
}

/** Every local address the Host could advertise, most useful first. */
export function listLanInterfaces(): Promise<readonly LanInterface[]> {
  return call<LanInterface[]>('list_lan_interfaces');
}

/**
 * Mint a join token and build its URL and QR code.
 *
 * The plaintext token exists only in the response: the backend stored a hash.
 * Issuing again invalidates the previous link (PRD section 22.2).
 */
export function issueJoinToken(
  meetingId: MeetingId,
  address: string,
  port: number,
): Promise<JoinTokenIssued> {
  return call<JoinTokenIssued>('issue_join_token', {
    meeting_id: meetingId,
    address,
    port,
  });
}

/**
 * Acknowledge a participant's claim.
 *
 * Records that the Host saw it. It grants nothing - the participant could
 * already take part, and still can (ADR-0016).
 */
export function approveParticipantClaim(
  meetingId: MeetingId,
  participantId: ParticipantId,
): Promise<SessionChanged> {
  return call<SessionChanged>('approve_participant_claim', {
    meeting_id: meetingId,
    participant_id: participantId,
  });
}

/**
 * End a participant's session, freeing the identity to be claimed again.
 *
 * What makes first-claim-wins workable in a room: a name taken by the wrong
 * person, or stranded on a closed laptop, can be released (ADR-0002 rule 5).
 */
export function revokeParticipantSession(
  meetingId: MeetingId,
  participantId: ParticipantId,
): Promise<SessionChanged> {
  return call<SessionChanged>('revoke_participant_session', {
    meeting_id: meetingId,
    participant_id: participantId,
  });
}

/**
 * Who is connected, and when each identity was last seen.
 *
 * `connected` is the LAN server's live count of open sockets, so everybody
 * reads as disconnected while the server is stopped - which is true, since
 * there is nothing to be connected to. `last_seen_at` is the durable half and
 * means the last moment a socket was observed to open or close; it is not a
 * heartbeat, so it can lag a silently dropped connection by up to one keepalive
 * interval (ADR-0018).
 */
export function listParticipantPresence(
  meetingId: MeetingId,
): Promise<readonly ParticipantPresence[]> {
  return call<ParticipantPresence[]>('list_participant_presence', {
    meeting_id: meetingId,
  });
}

/* -------------------------------------------------------------------------
 * Notes
 *
 * The Host may read and write any participant's note while the meeting is not
 * LOCKED (PRD section 15). Content is GFM-subset Markdown text and is
 * untrusted input even here: render it through `@lan-meeting/editor`, never
 * through an HTML sink.
 *
 * There is deliberately no restore. Version history is view-only in step 8
 * (ADR-0019), and no command writes a historical body back.
 * ------------------------------------------------------------------------- */

/** One participant's note, or `null` if they have not written one. */
export function getParticipantNote(
  meetingId: MeetingId,
  participantId: ParticipantId,
): Promise<NoteDetail | null> {
  return call<NoteDetail | null>('get_participant_note', {
    meeting_id: meetingId,
    participant_id: participantId,
  });
}

/**
 * Create or replace a participant's note.
 *
 * The version, the history row and the audit record are the backend's, derived
 * inside one transaction. Content rules - size, raw HTML, link schemes - are
 * enforced there too; the editor checks them first only so the Host is told
 * while typing rather than on save.
 */
export function writeParticipantNote(
  meetingId: MeetingId,
  participantId: ParticipantId,
  content: string,
): Promise<NoteWritten> {
  return call<NoteWritten>('write_participant_note', {
    meeting_id: meetingId,
    participant_id: participantId,
    content,
  });
}

/** A note's history, newest first, without bodies. */
export function listNoteVersions(
  meetingId: MeetingId,
  participantId: ParticipantId,
): Promise<readonly NoteVersionSummary[]> {
  return call<NoteVersionSummary[]>('list_note_versions', {
    meeting_id: meetingId,
    participant_id: participantId,
  });
}

/** One historical version, with its body. Read-only. */
export function getNoteVersion(
  meetingId: MeetingId,
  participantId: ParticipantId,
  version: number,
): Promise<NoteVersionDetail | null> {
  return call<NoteVersionDetail | null>('get_note_version', {
    meeting_id: meetingId,
    participant_id: participantId,
    version,
  });
}

/** Every participant, and whether they have written a note. */
export function listNotesOverview(
  meetingId: MeetingId,
): Promise<readonly NoteOverview[]> {
  return call<NoteOverview[]>('list_notes_overview', { meeting_id: meetingId });
}

/* -------------------------------------------------------------------------
 * Domain events
 * ------------------------------------------------------------------------- */

/**
 * Subscribe to every committed mutation in the backend.
 *
 * Resolves to an unsubscribe function. Tauri's listener is registered
 * asynchronously, so a caller that unmounts before it is ready must still be
 * able to cancel - which is why the returned promise is awaited and then
 * called, rather than the handle being assumed to exist.
 *
 * The handler receives events for **every** meeting, because the Host's window
 * shows more than one. Filtering by `meeting_id` is the caller's job and is a
 * display concern, not a security one: the Host may read every row of their own
 * database either way (ADR-0014).
 */
export async function onDomainEvent(
  handler: (event: HostDomainEvent) => void,
): Promise<() => void> {
  return listen<HostDomainEvent>(HOST_DOMAIN_EVENT, (message) => {
    handler(message.payload);
  });
}

/* -------------------------------------------------------------------------
 * Audit
 *
 * Read-only. There is no write, edit or delete counterpart, and there must not
 * be one: the log is append-only and the database refuses to change a row.
 * ------------------------------------------------------------------------- */

/** A meeting's audit trail, oldest first. */
export function listAuditEntries(meetingId: MeetingId): Promise<readonly AuditEntry[]> {
  return call<AuditEntry[]>('list_audit_entries', { meeting_id: meetingId });
}

export type { HostError };
