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
