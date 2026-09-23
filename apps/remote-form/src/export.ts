import { SUBMISSION_SCHEMA_VERSION } from '@lan-meeting/contracts';
import type { RemoteFormContext, RemoteSubmissionV1 } from '@lan-meeting/contracts';

/**
 * Turning a filled-in form into a submission file.
 *
 * Architecture rules section 20: a submission leaves this browser **only** when
 * the participant asks for it. There is no upload, no endpoint, no automatic
 * anything - the functions here are called from a click handler and from
 * nowhere else.
 *
 * Two ways out, because one of them is not reliable everywhere:
 *
 * | Route | Why it exists |
 * | --- | --- |
 * | a `Blob` download | what a participant expects: a file in their downloads |
 * | a copyable text box | browsers restrict downloads from `file://` unevenly |
 *
 * Both are entirely offline. A `Blob` URL is a handle to bytes already in this
 * page; nothing is sent anywhere, and nothing can be.
 */

/**
 * Build the submission for `context` and `note`, stamped now.
 *
 * Identity is copied from the context and never recomputed: `submission_id` is
 * the artefact identity the Host minted, and this form does not mint one
 * (ADR-0021 decision 7). `submitted_at` is this machine's clock, which is why
 * the Host treats it as display-only and excludes it from the canonical hash.
 */
export function buildSubmission(
  context: RemoteFormContext,
  note: string,
  now: Date = new Date(),
): RemoteSubmissionV1 {
  return {
    schema_version: SUBMISSION_SCHEMA_VERSION,
    submission_id: context.submission_id,
    meeting_id: context.meeting_id,
    participant_id: context.participant_id,
    participant_name: context.participant_name,
    source_version: context.source_version,
    generated_at: context.generated_at,
    submitted_at: now.toISOString() as RemoteSubmissionV1['submitted_at'],
    note,
  };
}

/**
 * The submission as the file's bytes.
 *
 * Indented, because a human may well open it: a submission that can be read
 * without a tool is easier to trust, and there is nothing secret in it.
 * Indentation does not affect the Host's canonical hash, which is computed over
 * the parsed fields rather than the file (ADR-0021 decision 8).
 */
export function serializeSubmission(submission: RemoteSubmissionV1): string {
  return `${JSON.stringify(submission, null, 2)}\n`;
}

/**
 * A filename, which is a convenience and nothing else.
 *
 * Architecture rules section 21: the application never uses a filename as an
 * identity or security mechanism. The Host reads the identifiers from inside
 * the file, so renaming this changes nothing.
 */
export function submissionFilename(context: RemoteFormContext): string {
  const slug = (text: string) =>
    text
      .normalize('NFKD')
      .replace(/[^A-Za-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 40);

  const meeting = slug(context.meeting_title) || 'meeting';
  const who = slug(context.participant_name) || 'participant';
  return `${meeting}-${who}-submission.json`;
}

/**
 * Offer `text` to the participant as a file download.
 *
 * Returns whether the attempt was made at all, so the caller can leave the
 * copyable fallback in view rather than claiming success it cannot observe. A
 * download is fire-and-forget: the browser may silently decline it, which is
 * exactly why the fallback is not hidden behind a failure.
 */
export function offerDownload(
  text: string,
  filename: string,
  doc: Document = document,
): boolean {
  try {
    const blob = new Blob([text], { type: 'application/json' });
    const url = URL.createObjectURL(blob);

    const anchor = doc.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.rel = 'noopener';
    anchor.style.display = 'none';

    doc.body.appendChild(anchor);
    anchor.click();
    anchor.remove();

    // Freed on the next turn, so the browser has taken the bytes first.
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
    return true;
  } catch {
    // A browser that refuses object URLs from `file://` is not an error state:
    // the participant still has the text in front of them.
    return false;
  }
}
