/**
 * The only module that talks to the LAN server.
 *
 * Five calls, matching the five routes the backend offers. Components go
 * through these functions and never call `fetch` directly, for the same reason
 * the Host UI centralizes `invoke`: a URL and a method are strings, and strings
 * repeated across components are typos waiting to become runtime failures.
 *
 * Nothing here decides anything. Whether the meeting is open, whether an
 * identity is free, whether a session may act - all of that is re-read inside a
 * database transaction on the Host (architecture rules section 15). This file
 * turns a call into a request and a rejection into a typed {@link LanError}.
 */

import type {
  LanClaimedView,
  LanError,
  LanJoinView,
  LanNoteView,
  LanSessionView,
  ParticipantId,
} from '@lan-meeting/contracts';

import { storedToken } from './session';

/** Whether a value is a backend {@link LanError}. */
export function isLanError(value: unknown): value is LanError {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const candidate = value as Partial<LanError>;
  return typeof candidate.kind === 'string'
    && typeof candidate.category === 'string'
    && typeof candidate.message === 'string';
}

/** A network failure, phrased for someone standing in a meeting room. */
function unreachable(): LanError {
  return {
    kind: 'internal',
    category: 'unexpected',
    message:
      'Could not reach the meeting host. Check that you are on the same '
      + 'network and that the host has not closed the meeting.',
  };
}

/**
 * Send one request, normalising any failure into a {@link LanError}.
 *
 * A non-2xx response carries a `LanError` body; anything else - the host's
 * laptop sleeping, the Wi-Fi dropping, a proxy interfering - becomes one here,
 * so no caller has to handle two shapes.
 */
async function call<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response;
  try {
    response = await fetch(path, {
      ...init,
      headers: {
        Accept: 'application/json',
        ...(init?.body === undefined ? {} : { 'Content-Type': 'application/json' }),
        ...(init?.headers ?? {}),
      },
    });
  } catch {
    throw unreachable();
  }

  let payload: unknown = null;
  try {
    payload = await response.json();
  } catch {
    // A body that is not JSON means something other than the API answered.
    if (!response.ok) {
      throw unreachable();
    }
  }

  if (!response.ok) {
    throw isLanError(payload) ? payload : unreachable();
  }

  return payload as T;
}

/** `Authorization: Bearer`, when a session token is held. */
function authorized(): HeadersInit {
  const token = storedToken();
  return token === null ? {} : { Authorization: `Bearer ${token}` };
}

/**
 * The meeting behind a join token, and the identities on offer.
 *
 * Needs no session: this is the screen someone sees before they have an
 * identity. What comes back is correspondingly minimal - names, and whether
 * each one is already taken.
 */
export function fetchJoin(token: string): Promise<LanJoinView> {
  return call<LanJoinView>(`/api/join/${encodeURIComponent(token)}`);
}

/**
 * Claim an identity.
 *
 * `participant_id` names *which* identity, not who is asking: the join token in
 * the path is the authority, and the backend decides who wins a race for a name
 * (ADR-0002).
 */
export function claimIdentity(
  token: string,
  participantId: ParticipantId,
): Promise<LanClaimedView> {
  return call<LanClaimedView>(`/api/join/${encodeURIComponent(token)}/claim`, {
    method: 'POST',
    body: JSON.stringify({ participant_id: participantId }),
  });
}

/**
 * Who the stored session token belongs to.
 *
 * Sends nothing but the credential. That is what makes reconnecting safe: there
 * is no parameter here that could select a different identity, so a reconnect
 * cannot become an identity change (ADR-0002 rule 10).
 */
export function fetchSession(): Promise<LanSessionView> {
  return call<LanSessionView>('/api/session', { headers: authorized() });
}

/**
 * The participant's own note, or `null` if they have not written one.
 *
 * Sends nothing but the credential. The backend derives the meeting and the
 * participant from the session row, so there is no parameter here through
 * which another participant's note could be requested - the question cannot be
 * expressed (ADR-0020).
 *
 * Available while the meeting is locked: a locked meeting is finished, not
 * secret, and the participant wrote this note.
 */
export function fetchOwnNote(): Promise<LanNoteView | null> {
  return call<LanNoteView | null>('/api/note', { headers: authorized() });
}

/**
 * Replace the participant's own note.
 *
 * The body carries `content` and nothing else. Identity is the session's, the
 * version is the backend's, and there is no `expected_version`: the write is
 * last-write-wins and what it replaces stays in history (ADR-0020).
 *
 * Whether the meeting still permits the write is re-read inside the backend's
 * own transaction, so a stale screen cannot talk this call into a write.
 */
export function writeOwnNote(content: string): Promise<LanNoteView> {
  return call<LanNoteView>('/api/note', {
    method: 'PUT',
    headers: authorized(),
    body: JSON.stringify({ content }),
  });
}
