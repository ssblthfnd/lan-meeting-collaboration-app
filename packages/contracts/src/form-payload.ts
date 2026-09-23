/**
 * The immutable metadata the Host bakes into a generated remote form.
 *
 * Architecture rules section 7 lists what every generated form must carry;
 * ADR-0021 decision 4 adds the participant's existing note so that a remote
 * submission is an *edit* rather than a silent rewrite. Those eleven fields are
 * the whole of it - this shape is frozen, and a field is added by amending the
 * decision record, not by widening the type.
 *
 * # Nothing here is authority
 *
 * The Host selected the meeting and the participant; this payload only
 * describes that choice to the form. When the submission comes back,
 * `meeting_id` and `participant_id` are **candidate identifiers** resolved by
 * lookup against the database, and `participant_name` is shown to a human for
 * verification and never matched against anything
 * (architecture rules sections 10 and 21, ADR-0021 decision 3).
 *
 * # Nothing here is a credential
 *
 * No join token, no session token, no token hash, no join URL, no port, no
 * host address. The generated file is intentionally not an authenticated
 * credential (ADR-0021 decision 3), and a boundary guard asserts the absence.
 */

import type {
  IanaTimeZone,
  IsoDate,
  Iso8601Utc,
  MeetingId,
  ParticipantId,
  SubmissionId,
} from './index';

/**
 * The element the payload travels in, inside the generated HTML.
 *
 * ```html
 * <script type="application/json" id="submission-context">{...}</script>
 * ```
 *
 * Read with `textContent` and `JSON.parse`. Never `innerHTML`: this bundle has
 * no HTML sink, and one inside a file that runs from `file://` would be a
 * sanitiser bypass no backend check could reach (ADR-0009, ADR-0021).
 */
export const SUBMISSION_CONTEXT_ELEMENT_ID = 'submission-context';

/**
 * What a generated remote form knows about itself.
 *
 * Produced by `app-remote` on the Host, consumed by `apps/remote-form` in a
 * participant's browser with no network of any kind.
 */
export interface RemoteFormContext {
  /** Always {@link SUBMISSION_SCHEMA_VERSION}. */
  readonly schema_version: number;

  /**
   * Identity of **this generated artifact**, minted by the Host as a UUIDv7.
   *
   * Baked in here and returned by the form unchanged. The form never mints
   * one. Generating a second form for the same participant produces a
   * different `submission_id`, and both remain importable (ADR-0021
   * decisions 7 and 13).
   */
  readonly submission_id: SubmissionId;

  readonly meeting_id: MeetingId;
  readonly meeting_title: string;

  /**
   * Meaningless without {@link RemoteFormContext.meeting_timezone}, which is
   * why the two always travel together (ADR-0005, PRD section 25.2).
   */
  readonly meeting_date: IsoDate;
  readonly meeting_timezone: IanaTimeZone;

  readonly participant_id: ParticipantId;
  readonly participant_name: string;

  /**
   * The note version current on the Host when this form was generated, or `0`
   * when the participant had no note.
   *
   * **Advisory only.** It is shown to the Host during the import preview and
   * is never a precondition: no comparison blocks an import, and nothing here
   * is optimistic concurrency (ADR-0021 decision 9, ADR-0019 decision 10).
   */
  readonly source_version: number;

  /**
   * The participant's note as it stood at generation, or `''` when they had
   * none.
   *
   * GFM-subset Markdown (ADR-0007). Editable form state, and nothing more: it
   * is never an authorization source, never an identity source, and never
   * evidence at import time.
   */
  readonly existing_content: string;

  /** When the Host generated this artifact. The Host's clock, and trusted. */
  readonly generated_at: Iso8601Utc;
}
