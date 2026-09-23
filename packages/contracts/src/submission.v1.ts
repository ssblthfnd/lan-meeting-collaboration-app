/**
 * The remote submission schema, version 1.
 *
 * Produced by `apps/remote-form` in a participant's browser, entirely offline,
 * and consumed by the Host as **untrusted input** (architecture rules section
 * 10). The application generated the form, but the file that comes back is
 * external data: the filename is never identity, a name inside the payload is
 * never identity, and valid JSON does not mean valid data.
 *
 * Frozen by ADR-0021. There is no `links` array: structured note links were
 * deferred by ADR-0019 and ADR-0020, and Markdown already carries links under
 * the same three-scheme allowlist (ADR-0021 decision 5).
 *
 * # The canonical form
 *
 * {@link canonicalSubmission} defines the exact bytes the Host hashes to decide
 * whether two submissions are the same artifact. It lives here, beside the
 * shape, because it *is* part of the shape: a byte-level contract two languages
 * have to agree on. `packages/contracts/__fixtures__/submissions.json` is read
 * at run time by both test suites, so a disagreement is a failing test naming
 * the row rather than a mystery at import time.
 *
 * It is deliberately not a business rule. What a submission *means*, whether it
 * may be imported and what it does to a note are decided in Rust, inside the
 * mutating transaction.
 */

import type {
  Iso8601Utc,
  MeetingId,
  ParticipantId,
  SubmissionId,
} from './index';

/**
 * What a participant's exported submission contains.
 *
 * Every field is untrusted on arrival. The two identifiers are **candidates**
 * resolved by lookup against the database; `participant_name` is displayed for
 * human verification and is never a match key.
 */
export interface RemoteSubmissionV1 {
  /** Must equal `SUBMISSION_SCHEMA_VERSION`. Any other value is refused. */
  readonly schema_version: number;

  /**
   * The artifact this submission was exported from, unchanged from the form.
   *
   * Duplicate semantics turn on it (ADR-0021 decision 7):
   *
   * | Seen before | Canonical hash | Outcome |
   * | --- | --- | --- |
   * | yes | same | duplicate, refused, no second note version |
   * | yes | different | the artifact was altered, refused |
   * | no | — | a distinct artifact |
   *
   * A correction is a **new form**, not a second export of an old one.
   */
  readonly submission_id: SubmissionId;

  readonly meeting_id: MeetingId;
  readonly participant_id: ParticipantId;

  /** Display and verification only. Never used to match a participant. */
  readonly participant_name: string;

  /** Advisory context for the Host's preview. Never a precondition. */
  readonly source_version: number;

  /** Copied from the form. The Host's clock at generation. */
  readonly generated_at: Iso8601Utc;

  /**
   * When the participant pressed *Export Submission*.
   *
   * A remote machine's clock, so **untrusted and display-only**. It orders
   * nothing, resolves no conflict, and is excluded from the canonical hash -
   * otherwise re-exporting an unchanged note would look like a different
   * artifact.
   */
  readonly submitted_at: Iso8601Utc;

  /** The participant's single note, GFM-subset Markdown (ADR-0003, ADR-0007). */
  readonly note: string;
}

/**
 * Normalise a note's line endings, exactly as the backend does.
 *
 * A browser can submit CRLF and the domain refuses a carriage return as a
 * control character. `app-server`'s note route and the Host's note command both
 * apply this same conversion at their boundary, so all three write paths store
 * identical bytes for identical typing.
 *
 * It is also what makes the canonical hash survive transport: a mail gateway
 * that rewrites line endings must not turn an unaltered submission into an
 * altered one.
 */
export function normalizeNote(note: string): string {
  return note.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
}

/**
 * The exact bytes the Host hashes, as a string.
 *
 * Deliberately **not** JSON. Two JSON serialisers agreeing byte-for-byte on
 * every escape, for every input a participant can type, is an assumption this
 * contract would rather not rest on - and the one field that is arbitrary text
 * is the one that decides the hash. So the note is length-prefixed with its
 * UTF-8 byte count and placed last, which makes its content impossible to
 * confuse with structure and leaves nothing for an escaping rule to disagree
 * about.
 *
 * Fields, in declaration order (ADR-0021 decision 8):
 *
 * ```text
 * lan-meeting/remote-submission/v1
 * schema_version=1
 * submission_id=<uuid>
 * meeting_id=<uuid>
 * participant_id=<uuid>
 * note=<utf-8 byte length>:<normalised note>
 * ```
 *
 * Excluded on purpose: `generated_at`, `submitted_at`, `participant_name` and
 * `source_version`. None of them says anything about what the note is, and
 * including a participant-controlled clock would make every export of an
 * unchanged note a different artifact.
 */
export function canonicalSubmission(
  submission: Pick<
    RemoteSubmissionV1,
    'schema_version' | 'submission_id' | 'meeting_id' | 'participant_id' | 'note'
  >,
): string {
  const note = normalizeNote(submission.note);
  const bytes = utf8Length(note);

  return [
    CANONICAL_PREFIX,
    `schema_version=${submission.schema_version}`,
    `submission_id=${submission.submission_id}`,
    `meeting_id=${submission.meeting_id}`,
    `participant_id=${submission.participant_id}`,
    `note=${bytes}:${note}`,
  ].join('\n');
}

/**
 * The first line of every canonical form.
 *
 * Domain-separates this hash from any other SHA-256 in the application, so a
 * digest computed over something else can never be mistaken for a submission's.
 */
export const CANONICAL_PREFIX = 'lan-meeting/remote-submission/v1';

/**
 * How many bytes `text` occupies as UTF-8.
 *
 * Counted arithmetically rather than with `TextEncoder`, so this package needs
 * no `DOM` library and no `@types/node`. That is not fussiness: this file is
 * imported by the offline form, by the Host UI and by a test runner, and a
 * contract that depends on which of those it is running in is a contract that
 * can disagree with itself.
 *
 * `for...of` iterates code points, so an astral character is counted once, as
 * four bytes. A lone surrogate counts as three, which is what an encoder would
 * emit for the replacement character it substitutes.
 */
function utf8Length(text: string): number {
  let bytes = 0;

  for (const character of text) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x80) {
      bytes += 1;
    } else if (code < 0x800) {
      bytes += 2;
    } else if (code < 0x10000) {
      bytes += 3;
    } else {
      bytes += 4;
    }
  }

  return bytes;
}
