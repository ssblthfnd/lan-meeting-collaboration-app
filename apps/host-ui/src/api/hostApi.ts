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
 */

import { invoke } from '@tauri-apps/api/core';
import type {
  AuditEntry,
  HostError,
  MeetingConfigurationInput,
  MeetingCreated,
  MeetingDetail,
  MeetingId,
  MeetingSummary,
  MeetingTransitioned,
  MeetingUpdated,
  ParticipantAdded,
  ParticipantDetailsInput,
  ParticipantId,
  ParticipantRemoved,
  ParticipantSummary,
  ParticipantUpdated,
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
