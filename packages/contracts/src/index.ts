/**
 * Shared contracts for the LAN Meeting Collaboration App.
 *
 * This package is the single TypeScript definition of every shape that crosses
 * a boundary:
 *
 * - `api.ts`            - LAN HTTP request/response DTOs        (Phase 1, step 4)
 * - `ws-events.ts`      - audience-scoped WebSocket events      (Phase 1, step 5)
 * - `form-payload.ts`   - immutable metadata baked into a remote form
 *                                                               (Phase 1, step 7)
 * - `submission.v1.ts`  - remote submission schema, versioned   (Phase 1, step 8)
 *
 * The Rust side implements the same shapes independently. To stop the two from
 * drifting, shared JSON fixtures under `__fixtures__/` are exercised by both
 * the TypeScript and the Rust test suites.
 *
 * Nothing here may describe behaviour, only shape. Authorization, validation
 * and business rules live in the Rust backend.
 */

/** Maximum participants per meeting (PRD section 7). */
export const MAX_PARTICIPANTS = 99;

/** Minimum participants per meeting (PRD section 7). */
export const MIN_PARTICIPANTS = 1;

/** Maximum links attached to a single note (PRD section 14). */
export const MAX_LINKS_PER_NOTE = 5;

/** Current remote submission schema version (PRD section 11). */
export const SUBMISSION_SCHEMA_VERSION = 1;

/** Meeting lifecycle status persisted in SQLite (PRD section 6). */
export type MeetingStatus = 'DRAFT' | 'OPEN' | 'LOCKED';

/** Audit and note-version actor discriminator (architecture rules 17, 18). */
export type ActorType = 'HOST' | 'PARTICIPANT' | 'REMOTE_IMPORT';

/**
 * Participant identity claim status under the first-claim-wins model
 * (PRD section 5.2.1).
 *
 * This is a **derived** value, not a stored column: the backend computes it
 * from `participant_sessions` so that there is only one source of truth for
 * whether an identity is currently bound
 * (see `docs/adr/0008-phase1-schema-refinements.md`).
 */
export type ParticipantClaimStatus =
  /** No session row exists for this participant. */
  | 'UNCLAIMED'
  /** A session exists that is awaiting Host approval. */
  | 'PENDING'
  /** An active session holds this identity (`revoked_at IS NULL`). */
  | 'CLAIMED'
  /** Every session for this participant is revoked. A Host rejection lands
   *  here too: rejecting a claim revokes its session. */
  | 'REVOKED';

/* -------------------------------------------------------------------------
 * Identifiers and time
 *
 * These mirror the Rust representation established in Phase 1, step 1. The
 * same 36-character id string and the same fixed-width UTC timestamp cross
 * every boundary: SQLite column, Tauri command, LAN HTTP body, and remote
 * submission file. See `docs/adr/0010-datetime-library-jiff.md` and
 * `docs/adr/0011-identifier-and-storage-formats.md`.
 *
 * They are branded string types. A brand costs nothing at runtime - the value
 * is a plain string in JSON - but it stops a `MeetingId` being passed where a
 * `ParticipantId` is expected, which is the same guarantee `Id<E>` gives on
 * the Rust side.
 *
 * Nothing here validates. Validation is a backend decision (architecture rules
 * section 3): a value arriving from a browser or a submission file is
 * untrusted until Rust has parsed it.
 * ------------------------------------------------------------------------- */

declare const brand: unique symbol;

/** A string tagged with a compile-time brand. */
type Branded<T extends string, B extends string> = T & { readonly [brand]: B };

/**
 * A UUID version 7, canonical lowercase hyphenated form, exactly 36
 * characters - for example `0199c7e1-5f2a-7b3c-8d4e-5f6a7b8c9d0e`.
 *
 * Version 7 is time-ordered, so sorting by id is also sorting by creation
 * time (architecture rules section 26.4).
 */
export type Uuid7<B extends string> = Branded<string, B>;

export type MeetingId = Uuid7<'MeetingId'>;
export type ParticipantId = Uuid7<'ParticipantId'>;
export type SessionId = Uuid7<'SessionId'>;
export type NoteId = Uuid7<'NoteId'>;
export type NoteLinkId = Uuid7<'NoteLinkId'>;
export type NoteVersionId = Uuid7<'NoteVersionId'>;
export type AuditLogId = Uuid7<'AuditLogId'>;
export type RemoteSubmissionId = Uuid7<'RemoteSubmissionId'>;

/**
 * The stable submission identity minted when a remote form is generated and
 * carried back inside the submission file (ADR-0008).
 */
export type SubmissionId = Uuid7<'SubmissionId'>;

/**
 * A system timestamp: UTC, RFC 3339, fixed width, always exactly
 * `YYYY-MM-DDTHH:MM:SS.sssZ` (24 characters).
 *
 * The milliseconds are always present even when zero, and the `Z` is always
 * literal. Both matter: fixed width is what makes string ordering agree with
 * time ordering, and the `Z` is what makes "this column is UTC" checkable
 * rather than assumed (PRD section 25.1).
 */
export type Iso8601Utc = Branded<string, 'Iso8601Utc'>;

/**
 * A meeting's calendar date, `YYYY-MM-DD`, with no timezone of its own.
 *
 * Meaningless without the meeting's {@link IanaTimeZone}. Never render it in
 * the reader's local timezone (PRD section 25.2).
 */
export type IsoDate = Branded<string, 'IsoDate'>;

/**
 * A meeting's time of day, `HH:MM:SS`, with no timezone of its own.
 *
 * Read in the meeting's {@link IanaTimeZone}, never the reader's.
 */
export type IsoTime = Branded<string, 'IsoTime'>;

/**
 * An IANA timezone identifier, for example `Asia/Makassar`.
 *
 * Stored explicitly on every meeting. The OS timezone may be offered as a
 * default when the Host creates a meeting, but is never used implicitly to
 * decide what a stored schedule means (ADR-0005).
 */
export type IanaTimeZone = Branded<string, 'IanaTimeZone'>;

/** Resolution recorded for a processed remote submission (ADR-0008). */
export type SubmissionResolution =
  | 'IMPORTED'
  | 'REPLACED'
  | 'REJECTED_DUPLICATE'
  | 'REJECTED_INVALID';
