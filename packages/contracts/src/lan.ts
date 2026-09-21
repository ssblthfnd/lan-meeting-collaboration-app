/**
 * The LAN boundary: shapes crossing between `apps/lan-ui` and `app-server`.
 *
 * A separate file from `host.ts`, and deliberately not a subset of it. The Host
 * and a participant are different audiences: the Host may see everything in
 * their own database and a participant may not, and a type shared across both is
 * how something meant for one ends up in the other's hands
 * (ADR-0014, architecture rules section 16).
 *
 * Read these types as a list of what a participant is told. There is no note, no
 * audit entry, no participant count, no other person's department, no token hash
 * and no session id anywhere below - not because a query filters them out, but
 * because no shape here has a field to put them in.
 *
 * # Two visibility tiers
 *
 * | Holding | May see |
 * | --- | --- |
 * | a join token | the meeting's public facts; names and whether they are taken |
 * | a session token | the above, plus their own full details |
 *
 * A name is unavoidable, because a participant picks their own identity from the
 * list (PRD section 5.2). Department, position and role are not needed for that,
 * so they are withheld until the claim succeeds (ADR-0016).
 */

import type {
  IanaTimeZone,
  IsoDate,
  IsoTime,
  MeetingId,
  MeetingStatus,
  ParticipantId,
} from './index';

/* -------------------------------------------------------------------------
 * Responses
 * ------------------------------------------------------------------------- */

/** The meeting, as a joining participant may see it. */
export interface LanMeetingView {
  readonly id: MeetingId;
  readonly title: string;
  readonly topic: string | null;
  /** Zoneless. Meaningless without {@link LanMeetingView.timezone}. */
  readonly date: IsoDate;
  readonly start_time: IsoTime;
  readonly end_time: IsoTime;
  /**
   * Always sent with the schedule.
   *
   * A participant's phone may be in another timezone; the meeting's hours are
   * the meeting's, and the UI must render them in this zone rather than the
   * device's (PRD section 25.3).
   */
  readonly timezone: IanaTimeZone;
  readonly location: string | null;
  readonly status: MeetingStatus;
}

/**
 * One identity on the join screen.
 *
 * `claimed` is a boolean rather than the Host's four-state claim status:
 * whether the Host has acknowledged somebody else's claim is the Host's
 * business, and a participant only needs to know which names are still free.
 * A pending claim and an acknowledged one are equally taken (ADR-0016).
 */
export interface LanIdentityView {
  readonly id: ParticipantId;
  readonly name: string;
  readonly claimed: boolean;
}

/** What `GET /api/join/{token}` returns. */
export interface LanJoinView {
  readonly meeting: LanMeetingView;
  readonly identities: readonly LanIdentityView[];
}

/** The participant's own identity, after claiming it. */
export interface LanOwnIdentityView {
  readonly id: ParticipantId;
  readonly name: string;
  readonly department: string | null;
  readonly position: string | null;
  readonly meeting_role: string | null;
}

/**
 * What a successful claim returns.
 *
 * The only response in the application carrying a session token. It arrives
 * once, and only the browser that earned it ever sees it - the backend stores a
 * hash (PRD section 22.17).
 */
export interface LanClaimedView {
  readonly session_token: string;
  readonly participant: LanOwnIdentityView;
  readonly meeting: LanMeetingView;
}

/**
 * What `GET /api/session` returns.
 *
 * No session token: reconnecting proves you already hold one.
 */
export interface LanSessionView {
  readonly participant: LanOwnIdentityView;
  readonly meeting: LanMeetingView;
  /**
   * Whether the Host has acknowledged the claim.
   *
   * **Display only.** A participant with `false` can do everything a
   * participant with `true` can: approval is acknowledgement, not a permission
   * gate (ADR-0016). A UI must never disable an action because of this.
   */
  readonly acknowledged: boolean;
}

/* -------------------------------------------------------------------------
 * Requests
 * ------------------------------------------------------------------------- */

/**
 * Which identity to claim.
 *
 * `participant_id` is a **target**, not a statement of who is asking. Authority
 * comes from the join token in the URL, and who wins a race for an identity is
 * decided by the database, not by this field
 * (architecture rules section 14.1 rule 5).
 */
export interface LanClaimRequest {
  readonly participant_id: ParticipantId;
}

/* -------------------------------------------------------------------------
 * Errors
 * ------------------------------------------------------------------------- */

/** The coarse grouping a participant UI branches on. */
export type LanErrorCategory =
  | 'authorization'
  | 'not_found'
  | 'lifecycle'
  | 'validation'
  | 'conflict'
  | 'unexpected';

/**
 * A refusal from the LAN API.
 *
 * Shares a vocabulary with {@link HostError} and shares no fields with it. Two
 * differences are deliberate and load-bearing:
 *
 * - **No identifiers.** A refusal that echoed a meeting or participant id would
 *   confirm to whoever provoked it that the id is real. An unknown join token
 *   and an unknown route return the same `not_found`.
 * - **The message is written for a participant**, not taken from the domain -
 *   the domain's own wording names internal ids and is aimed at the Host.
 */
export interface LanError {
  readonly kind:
    | 'not_found'
    | 'meeting_not_open'
    | 'identity_already_claimed'
    | 'unauthenticated'
    | 'forbidden'
    | 'invalid_request'
    | 'internal';
  readonly category: LanErrorCategory;
  readonly message: string;
}
