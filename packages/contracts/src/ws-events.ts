/**
 * Domain events, as the two audiences receive them.
 *
 * An event is a **notification**, never a carrier of state. It says that
 * something committed and names the identifiers needed to go and re-read it.
 * There is no note content here, no participant details, no credential and no
 * token hash - a client that may read the thing that changed asks the HTTP or
 * command boundary for it, where the audience rules are applied again
 * (architecture rules section 16).
 *
 * That is also why nothing here is authoritative. SQLite is the source of
 * truth; the socket is a hint to go and look. A client that misses an event is
 * wrong for as long as it takes to refetch, and a client that reconnects
 * refetches anyway - which is why there is no replay, no acknowledgement and no
 * sequence number to reason about (ADR-0018).
 *
 * # Two audiences, two shapes
 *
 * | Audience | Transport | Type |
 * | --- | --- | --- |
 * | LAN participant | WebSocket at `/ws` | {@link LanEvent} |
 * | Host | Tauri IPC, event name `domain-event` | {@link HostDomainEvent} |
 *
 * They are separate for the reason every other boundary type here is separate
 * (ADR-0014): the Host may see everything in their own database and a
 * participant may not. {@link LanEvent} is a union of the four kinds a
 * participant can ever receive, so a participant handler that tried to read a
 * roster event would not compile - the audience decision is enforced in Rust,
 * and the type simply describes what survives it.
 */

import type {
  Iso8601Utc,
  MeetingId,
  NoteId,
  ParticipantId,
  SessionId,
} from './index';

/* -------------------------------------------------------------------------
 * Kinds
 * ------------------------------------------------------------------------- */

/**
 * Every event the domain publishes.
 *
 * The strings are `subject.verb` and are shared verbatim with the Rust
 * `DomainEvent::kind`.
 */
export type DomainEventKind =
  | 'meeting.created'
  | 'meeting.updated'
  | 'meeting.opened'
  | 'meeting.locked'
  | 'meeting.join_token_issued'
  | 'participant.added'
  | 'participant.updated'
  | 'participant.removed'
  | 'participant.claimed'
  | 'claim.approved'
  | 'session.revoked'
  | 'presence.changed'
  | 'note.changed';

/* -------------------------------------------------------------------------
 * The participant socket
 * ------------------------------------------------------------------------- */

/**
 * The subprotocol tag a participant bundle offers, alongside its session token.
 *
 * ```ts
 * new WebSocket(url, [LAN_SOCKET_SUBPROTOCOL, sessionToken]);
 * ```
 *
 * The browser WebSocket API cannot set a request header, so this is how the
 * credential reaches the server without going in the URL - where it would end
 * up in logs, `Referer` headers and browser history (ADR-0018). The order is
 * fixed: the tag first, the credential second.
 */
export const LAN_SOCKET_SUBPROTOCOL = 'lan-meeting.v1';

/**
 * Close codes a participant bundle must tell apart.
 *
 * Both are from the private-use range, because no registered code means "your
 * credential was withdrawn" or "you missed events" - and the two call for
 * opposite responses.
 */
export const LAN_SOCKET_CLOSE = {
  /** The host ended this session. Forget the token; do not reconnect. */
  SESSION_REVOKED: 4401,
  /** Events were dropped for this socket. Reconnect and refetch everything. */
  RESYNC_REQUIRED: 4408,
} as const;

/** Fields every participant frame carries. */
interface LanEventBase {
  readonly meeting_id: MeetingId;
  /** When the mutation committed, in UTC. */
  readonly at: Iso8601Utc;
}

/**
 * The meeting was locked.
 *
 * The only lifecycle event a participant receives, and the only one that
 * changes what their screen should offer. It does **not** change what they are
 * allowed to do: the backend re-reads the meeting's status inside every
 * mutating transaction, so a participant who never receives this is refused
 * exactly as firmly as one who does (architecture rules section 15). A UI must
 * treat it as a cue to refetch, never as the enforcement.
 */
export interface MeetingLockedEvent extends LanEventBase {
  readonly type: 'meeting.locked';
}

/**
 * The host acknowledged this participant's claim.
 *
 * Display state. It grants nothing - the participant could already take part,
 * and still can (ADR-0016).
 */
export interface ClaimApprovedEvent extends LanEventBase {
  readonly type: 'claim.approved';
  readonly participant_id: ParticipantId;
}

/**
 * This participant's note changed, and history gained a version.
 *
 * Carries the version and never the Markdown. The content is fetched through
 * the boundary that decides who may read it.
 */
export interface NoteChangedEvent extends LanEventBase {
  readonly type: 'note.changed';
  readonly participant_id: ParticipantId;
  readonly note_id: NoteId;
  /** Starts at 1 and increases by one per write. */
  readonly version: number;
}

/**
 * The host ended the session this socket authenticated with.
 *
 * Delivered to that one socket, immediately before it is closed with
 * {@link LAN_SOCKET_CLOSE.SESSION_REVOKED}. The same participant's other
 * devices, holding other sessions, are untouched (ADR-0002 rule 5).
 */
export interface SessionRevokedEvent extends LanEventBase {
  readonly type: 'session.revoked';
  readonly participant_id: ParticipantId;
  readonly session_id: SessionId;
}

/**
 * Everything a LAN participant can be sent.
 *
 * Four kinds, and no others: roster changes, presence and the rest of the
 * lifecycle are the host's and are filtered out in Rust before they reach a
 * socket. A participant never receives another participant's event.
 */
export type LanEvent =
  | MeetingLockedEvent
  | ClaimApprovedEvent
  | NoteChangedEvent
  | SessionRevokedEvent;

/* -------------------------------------------------------------------------
 * The Host window
 * ------------------------------------------------------------------------- */

/** The Tauri event name every domain event arrives under. */
export const HOST_DOMAIN_EVENT = 'domain-event';

/**
 * What the Host window receives, for any of the thirteen kinds.
 *
 * One wide shape rather than a union, because the Host subscribes once and
 * routes on `type`: a union would make the common case - "something in this
 * meeting changed, reload" - the awkward one.
 *
 * The optional fields are absent rather than `null` when they do not apply, so
 * a frame carries only what its kind actually means.
 */
export interface HostDomainEvent {
  readonly type: DomainEventKind;
  readonly meeting_id: MeetingId;
  readonly participant_id?: ParticipantId;
  /** Present on `session.revoked` alone. */
  readonly session_id?: SessionId;
  readonly note_id?: NoteId;
  readonly version?: number;
  /** Present on `presence.changed` alone. */
  readonly connected?: boolean;
  readonly at: Iso8601Utc;
}
