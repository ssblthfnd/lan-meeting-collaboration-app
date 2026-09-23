/**
 * The Host boundary: shapes crossing between the Host UI and the Tauri
 * commands in `src-tauri`.
 *
 * This is the contract for one specific transport - Tauri IPC inside the Host
 * window. It is deliberately not shared with the LAN transport: `apps/lan-ui`
 * talks to `app-server` over HTTP and gets its own contract, because a
 * participant must never be handed the shapes the Host sees
 * (architecture rules section 16).
 *
 * # Inputs are plain, outputs are branded
 *
 * Every identifier, timestamp, date, time and timezone that *leaves* the
 * backend is a branded type, because the backend is the only thing that can
 * produce a valid one. Everything the UI *sends* is a plain `string`: a value
 * typed into a form is untrusted text until Rust has parsed it, and pretending
 * otherwise in the type system would be exactly the "UI validation as
 * enforcement" that architecture rules section 3 forbids.
 *
 * That asymmetry is load-bearing rather than cosmetic. A `MeetingId` can only
 * be obtained from a backend response, so no UI code can invent one and no cast
 * is needed to pass one back.
 *
 * Nothing here describes behaviour. Authorization, the meeting lock, the
 * DRAFT-only rule and the 99-participant limit are decided in `app-core`
 * (ADR-0012, ADR-0013); the UI only renders what it is told.
 */

import type {
  ActorType,
  IanaTimeZone,
  Iso8601Utc,
  IsoDate,
  IsoTime,
  MeetingId,
  MeetingStatus,
  NoteId,
  ParticipantClaimStatus,
  ParticipantId,
  SubmissionId,
} from './index';

/* -------------------------------------------------------------------------
 * Audit
 * ------------------------------------------------------------------------- */

/**
 * Every audit action the backend can currently write.
 *
 * A closed union rather than `string`, so a view that switches on the action
 * fails to compile when the backend gains one. The values are the persisted
 * `audit_logs.action` strings and are stable.
 */
export type AuditAction =
  | 'meeting.created'
  | 'meeting.updated'
  | 'meeting.opened'
  | 'meeting.locked'
  | 'participant.added'
  | 'participant.updated'
  | 'participant.removed'
  | 'meeting.join_token_issued'
  | 'participant.claimed'
  | 'participant.claim_approved'
  | 'participant.session_revoked'
  | 'note.created'
  | 'note.updated';

/** What an audit record was about. */
export type AuditTargetType = 'meeting' | 'participant' | 'session' | 'note';

/**
 * One audit record, as the Host audit view receives it.
 *
 * Read-only in the strongest sense: the log is append-only and the database
 * refuses `UPDATE` and `DELETE` outright (ADR-0011). There is no command to
 * change one, and there must never be a UI action that offers to.
 */
export interface AuditEntry {
  readonly id: string;
  readonly actor_type: ActorType;
  /** `null` exactly when `actor_type` is `HOST`, which is not a participant. */
  readonly actor_id: ParticipantId | null;
  readonly action: AuditAction;
  readonly target_type: AuditTargetType;
  readonly target_id: string | null;
  /** Parsed from the stored JSON object, or `null` when the record has none. */
  readonly metadata: Readonly<Record<string, unknown>> | null;
  readonly created_at: Iso8601Utc;
}

/* -------------------------------------------------------------------------
 * Meetings
 * ------------------------------------------------------------------------- */

/** A meeting as it appears in the Host's meeting list. */
export interface MeetingSummary {
  readonly id: MeetingId;
  readonly title: string;
  readonly topic: string | null;
  /** Read in {@link MeetingSummary.timezone}, never in the reader's zone. */
  readonly date: IsoDate;
  readonly start_time: IsoTime;
  readonly end_time: IsoTime;
  readonly timezone: IanaTimeZone;
  readonly status: MeetingStatus;
  readonly participant_count: number;
  readonly created_at: Iso8601Utc;
}

/** Everything the Host's meeting detail view shows. */
export interface MeetingDetail {
  readonly id: MeetingId;
  readonly title: string;
  readonly topic: string | null;
  readonly date: IsoDate;
  readonly start_time: IsoTime;
  readonly end_time: IsoTime;
  readonly timezone: IanaTimeZone;
  readonly location: string | null;
  readonly description: string | null;
  readonly status: MeetingStatus;
  readonly participant_count: number;
  readonly created_at: Iso8601Utc;
  readonly updated_at: Iso8601Utc;
  /** Set exactly when `status` is `LOCKED`. */
  readonly locked_at: Iso8601Utc | null;
}

/**
 * What the Host submits when creating or reconfiguring a meeting
 * (PRD section 7).
 *
 * All strings: the backend parses and validates. `timezone` is required - a
 * meeting whose timezone is unknown has an ambiguous schedule (ADR-0005) - and
 * the OS timezone may only be *offered* as a default here, never assumed later.
 */
export interface MeetingConfigurationInput {
  readonly title: string;
  readonly topic: string | null;
  /** `YYYY-MM-DD`. */
  readonly date: string;
  /** `HH:MM:SS`. */
  readonly start_time: string;
  /** `HH:MM:SS`. */
  readonly end_time: string;
  /** An IANA identifier, for example `Asia/Makassar`. */
  readonly timezone: string;
  readonly location: string | null;
  readonly description: string | null;
}

/** Outcome of creating a meeting. Always `DRAFT` (ADR-0013). */
export interface MeetingCreated {
  readonly meeting_id: MeetingId;
  readonly status: MeetingStatus;
  readonly at: Iso8601Utc;
}

/** Outcome of reconfiguring a meeting. */
export interface MeetingUpdated {
  readonly meeting_id: MeetingId;
  readonly at: Iso8601Utc;
}

/** Outcome of a lifecycle transition. */
export interface MeetingTransitioned {
  readonly meeting_id: MeetingId;
  readonly from: MeetingStatus;
  readonly to: MeetingStatus;
  readonly at: Iso8601Utc;
}

/* -------------------------------------------------------------------------
 * Participants
 * ------------------------------------------------------------------------- */

/**
 * One participant of a meeting.
 *
 * An identity the Host prepared, not an account and not a credential. Whether
 * someone is currently holding it is a separate, derived question, answered by
 * {@link ParticipantSummary.claim_status} (ADR-0008).
 */
export interface ParticipantSummary {
  readonly id: ParticipantId;
  readonly name: string;
  readonly department: string | null;
  readonly position: string | null;
  readonly meeting_role: string | null;
  /**
   * Derived from `participant_sessions`, never stored (ADR-0008).
   *
   * `PENDING` means someone has joined and is working - **not** that they are
   * waiting for permission. Approval is acknowledgement, not a gate, so a UI
   * must not present `PENDING` as something to be unblocked (ADR-0016).
   */
  readonly claim_status: ParticipantClaimStatus;
  readonly created_at: Iso8601Utc;
}

/* -------------------------------------------------------------------------
 * Notes
 *
 * Content is GFM-subset Markdown text (ADR-0007, ADR-0019). It is untrusted
 * input even in the Host's own window, and must be rendered through
 * `@lan-meeting/editor`, which builds DOM nodes rather than markup. Nothing in
 * the Host UI may put it through an HTML sink.
 * ------------------------------------------------------------------------- */

/** A participant's current note. */
export interface NoteDetail {
  readonly note_id: NoteId;
  readonly participant_id: ParticipantId;
  /** GFM-subset Markdown. Render it; never assign it as HTML. */
  readonly content: string;
  /** Newest history version. Dense from 1, so it is also the version count. */
  readonly version: number;
  readonly created_at: Iso8601Utc;
  readonly updated_at: Iso8601Utc;
  /** Who wrote the current content. */
  readonly last_author_type: ActorType;
  /** `null` exactly when `last_author_type` is `HOST` (ADR-0008). */
  readonly last_author_id: ParticipantId | null;
}

/**
 * One row of note history, without its body.
 *
 * A history list is metadata. The body of the one version being previewed is
 * fetched separately through {@link NoteVersionDetail}, so opening the history
 * tab does not move a note's entire past across the boundary.
 */
export interface NoteVersionSummary {
  readonly version: number;
  readonly created_at: Iso8601Utc;
  readonly created_by_type: ActorType;
  /** `null` exactly when `created_by_type` is `HOST`. */
  readonly created_by: ParticipantId | null;
  /** Size in bytes, the same unit the 64 KiB backend limit uses. */
  readonly byte_length: number;
}

/**
 * One historical version, with its body.
 *
 * Read-only in every sense: history is never rewritten (the database refuses
 * `UPDATE` on it), and step 8 has no restore - there is no command that writes
 * a historical body back (ADR-0019).
 */
export interface NoteVersionDetail {
  readonly version: number;
  readonly content: string;
  readonly created_at: Iso8601Utc;
  readonly created_by_type: ActorType;
  readonly created_by: ParticipantId | null;
}

/** What a successful note write returns. */
export interface NoteWritten {
  readonly note_id: NoteId;
  readonly version: number;
  /** True when the write created the note rather than replacing its content. */
  readonly created: boolean;
  readonly at: Iso8601Utc;
}

/**
 * One participant's note status, for the Host's notes list.
 *
 * Every participant appears, including those with no note: the list is the
 * roster, and hiding the people who have not written yet would hide exactly
 * the ones the Host is looking for.
 */
export interface NoteOverview {
  readonly participant_id: ParticipantId;
  readonly name: string;
  readonly note_id: NoteId | null;
  readonly version: number | null;
  readonly updated_at: Iso8601Utc | null;
}

/**
 * One participant's presence, as the Host's roster shows it.
 *
 * Two values from two places, deliberately. `connected` is the LAN server's
 * in-memory count of open sockets and is `false` for everybody while that
 * server is stopped - which is the truth, not a fallback. `last_seen_at` is the
 * durable half, and it means **the last moment a socket for the live session
 * was observed to open or close**: it is not a heartbeat, so a participant who
 * has been connected and quiet for an hour still shows the moment they
 * connected, and a connection that dropped without a close frame is noticed
 * only when the server's keepalive notices it.
 *
 * Presence is an observation and grants no authority. Nothing in the backend
 * consults it to decide what anybody may do.
 */
export interface ParticipantPresence {
  readonly participant_id: ParticipantId;
  readonly connected: boolean;
  readonly last_seen_at: Iso8601Utc | null;
}

/**
 * What the Host submits for a participant (PRD section 7).
 *
 * `name` is required. It is deliberately not unique within a meeting: identity
 * is the participant id, and a name is data rather than authority
 * (ADR-0013, architecture rules section 14.1).
 */
export interface ParticipantDetailsInput {
  readonly name: string;
  readonly department: string | null;
  readonly position: string | null;
  readonly meeting_role: string | null;
}

/** Outcome of adding a participant. */
export interface ParticipantAdded {
  readonly participant_id: ParticipantId;
  /** Roster size after the addition; never more than `MAX_PARTICIPANTS`. */
  readonly roster_size: number;
  readonly at: Iso8601Utc;
}

/** Outcome of changing a participant's details. */
export interface ParticipantUpdated {
  readonly participant_id: ParticipantId;
  readonly at: Iso8601Utc;
}

/** Outcome of removing a participant. */
export interface ParticipantRemoved {
  readonly participant_id: ParticipantId;
  readonly roster_size: number;
  readonly at: Iso8601Utc;
}

/* -------------------------------------------------------------------------
 * Errors
 *
 * See `docs/adr/0015-host-error-contract.md`.
 * ------------------------------------------------------------------------- */

/**
 * The coarse grouping a UI can branch on without knowing every `kind`.
 *
 * `kind` is the precise fact and is what a message should be built from;
 * `category` is what decides *how* to present it - an inline field message, a
 * refusal notice, a retry prompt.
 */
export type HostErrorCategory =
  /** The actor may not do this. Never a reason to retry. */
  | 'authorization'
  /** The meeting or participant asked for does not exist. */
  | 'not_found'
  /** The meeting's status forbids it: locked, or no longer `DRAFT`. */
  | 'lifecycle'
  /** The input was refused. `field` names what to correct. */
  | 'validation'
  /** A concurrent change won. Re-reading and retrying may succeed. */
  | 'conflict'
  /** A failure the backend could not interpret. A bug or an operational fault. */
  | 'unexpected';

/**
 * A structured error from a Host command.
 *
 * Discriminated on `kind`, so narrowing gives exactly the fields that kind
 * carries. `message` is the backend's own actionable sentence, which names the
 * expected and the detected value wherever that information exists
 * (architecture rules section 22) - a UI should prefer it over composing its own.
 *
 * Deliberately carries no SQLite diagnostics and no HTTP status codes.
 */
export type HostError =
  | {
      readonly kind: 'unauthorized';
      readonly category: 'authorization';
      readonly message: string;
      readonly meeting_id: MeetingId;
    }
  | {
      readonly kind: 'forbidden';
      readonly category: 'authorization';
      readonly message: string;
      readonly actor_type: ActorType;
      readonly action: string;
      readonly target: string;
    }
  | {
      readonly kind: 'meeting_not_found';
      readonly category: 'not_found';
      readonly message: string;
      readonly meeting_id: MeetingId;
    }
  | {
      readonly kind: 'meeting_locked';
      readonly category: 'lifecycle';
      readonly message: string;
      readonly meeting_id: MeetingId;
      readonly title: string;
    }
  | {
      readonly kind: 'meeting_not_draft';
      readonly category: 'lifecycle';
      readonly message: string;
      readonly meeting_id: MeetingId;
      /** The status actually found. */
      readonly detected: MeetingStatus;
    }
  | {
      readonly kind: 'meeting_not_open';
      readonly category: 'lifecycle';
      readonly message: string;
      readonly meeting_id: MeetingId;
      /** The status actually found. */
      readonly detected: MeetingStatus;
    }
  | {
      readonly kind: 'identity_already_claimed';
      readonly category: 'conflict';
      readonly message: string;
      readonly meeting_id: MeetingId;
      /** Which identity, so the roster can point at the right row. */
      readonly participant_id: ParticipantId;
    }
  | {
      readonly kind: 'session_not_found';
      readonly category: 'not_found';
      readonly message: string;
      readonly meeting_id: MeetingId;
    }
  | {
      readonly kind: 'participant_not_found';
      readonly category: 'not_found';
      readonly message: string;
      readonly meeting_id: MeetingId;
      readonly participant_id: ParticipantId;
    }
  | {
      readonly kind: 'invalid_transition';
      readonly category: 'lifecycle';
      readonly message: string;
      readonly expected: string;
      readonly detected: MeetingStatus;
    }
  | {
      readonly kind: 'validation';
      readonly category: 'validation';
      readonly message: string;
      /** Which input was refused, for attaching the message to a form field. */
      readonly field: string;
      readonly expected: string;
      readonly detected: string;
    }
  | {
      readonly kind: 'conflict';
      readonly category: 'conflict';
      readonly message: string;
    }
  | {
      readonly kind: 'persistence';
      readonly category: 'unexpected';
      readonly message: string;
    };

/* -------------------------------------------------------------------------
 * LAN access
 *
 * The Host's side of the LAN server: starting it, choosing which address to
 * advertise, and issuing the join token. What a *participant* sees is in
 * `lan.ts` and is deliberately a different set of shapes (ADR-0014).
 * ------------------------------------------------------------------------- */

/** Whether the LAN server is running, and where. */
export interface LanServerStatus {
  readonly running: boolean;
  /** The port actually bound, which is not always the one requested. */
  readonly port: number | null;
  /**
   * Whether the participant bundle was compiled into this binary.
   *
   * Surfaced so a developer running a Rust-only build is told why the join page
   * is blank, instead of debugging the network.
   */
  readonly ui_bundled: boolean;
}

/**
 * One local address the Host could advertise.
 *
 * The Host chooses: a laptop may be on Wi-Fi, Ethernet and a VPN at once, and
 * only the person in the room knows which network the participants are on.
 * `127.0.0.1` must never be assumed reachable by a participant
 * (architecture rules section 4), which is what `is_loopback` is for.
 */
export interface LanInterface {
  readonly name: string;
  readonly address: string;
  readonly is_loopback: boolean;
  readonly is_private: boolean;
}

/**
 * A QR code as a square grid of modules.
 *
 * `modules` is row-major and exactly `size * size` long; `true` is a dark
 * square. A grid rather than an image or an SVG string, so the UI draws
 * `<rect>` elements and never injects markup (ADR-0017).
 */
export interface QrMatrix {
  readonly size: number;
  readonly modules: readonly boolean[];
}

/**
 * A freshly issued join token, as a URL and a QR code.
 *
 * The plaintext token appears here and nowhere else: the backend stored only
 * its hash (PRD section 22.2). Issuing again produces a different one and
 * invalidates this, which is what `replaced_previous` warns about.
 */
export interface JoinTokenIssued {
  readonly meeting_id: MeetingId;
  readonly join_url: string;
  readonly qr: QrMatrix;
  readonly replaced_previous: boolean;
  readonly at: Iso8601Utc;
}

/**
 * Where a generated remote form was written, and what identifies it.
 *
 * The Host sends the file at `path` to the participant however they like.
 * Nothing in this application sends it anywhere (PRD section 10).
 *
 * `submission_id` is the artefact's identity: it makes re-importing the same
 * file a detected duplicate, and it tells two forms generated for the same
 * person apart (ADR-0021 decision 7).
 */
export interface RemoteFormGenerated {
  readonly meeting_id: MeetingId;
  readonly participant_id: ParticipantId;
  readonly submission_id: SubmissionId;
  /** Absolute path of the file the backend wrote. Chosen by the backend. */
  readonly path: string;
  /** Its name alone. A convenience, never an identity (rules section 21). */
  readonly file_name: string;
  readonly bytes: number;
  /** The note version the form was generated from, or 0. Advisory. */
  readonly source_version: number;
  readonly generated_at: Iso8601Utc;
}

/** Outcome of a Host action on a participant's session. */
export interface SessionChanged {
  readonly participant_id: ParticipantId;
  readonly at: Iso8601Utc;
}
