import {
  SUBMISSION_CONTEXT_ELEMENT_ID,
  SUBMISSION_SCHEMA_VERSION,
} from '@lan-meeting/contracts';
import type { RemoteFormContext } from '@lan-meeting/contracts';

/**
 * Reading the payload the Host baked into this file.
 *
 * ```html
 * <script type="application/json" id="submission-context">{...}</script>
 * ```
 *
 * Read through `textContent` and `JSON.parse`, never `innerHTML`. This bundle
 * has no HTML sink at all (ADR-0009), and one inside a file that runs from
 * `file://` would be a sanitiser bypass with no server, no CSP and no backend
 * check behind it.
 *
 * The Host escapes every `<` in the payload before injecting it, so nothing in
 * the context can close this element. That is asserted on the Rust side, where
 * the artefact is generated and verified before it is ever written to disk.
 *
 * # The unfilled template is a real state
 *
 * `apps/remote-form/dist/index.html` is the *template*: its island holds `null`
 * until the Host generates a form from it. Someone opening the template
 * directly - a developer running `npm run dev:remote`, or a participant sent the
 * wrong file - should be told that, not shown an empty form that produces a
 * submission for nobody.
 */

/** Why this file cannot be filled in. */
export type ContextProblem =
  /** The island is missing: this is not a generated form at all. */
  | { readonly kind: 'absent' }
  /** The island holds `null`: the template, not a generated form. */
  | { readonly kind: 'template' }
  /** The island is not readable, or is not a context of a version we know. */
  | { readonly kind: 'unreadable'; readonly detail: string };

export type ContextResult =
  | { readonly ok: true; readonly context: RemoteFormContext }
  | { readonly ok: false; readonly problem: ContextProblem };

/** Every field the Host bakes in, and the type each must have. */
const SHAPE = {
  schema_version: 'number',
  submission_id: 'string',
  meeting_id: 'string',
  meeting_title: 'string',
  meeting_date: 'string',
  meeting_timezone: 'string',
  participant_id: 'string',
  participant_name: 'string',
  source_version: 'number',
  existing_content: 'string',
  generated_at: 'string',
} as const;

/**
 * Read this file's own context.
 *
 * Checked rather than trusted. The Host wrote the payload, but a file that
 * travelled by email and back can arrive damaged, and showing a participant a
 * form assembled from half a payload would waste their time twice.
 */
export function readContext(doc: Document = document): ContextResult {
  const element = doc.getElementById(SUBMISSION_CONTEXT_ELEMENT_ID);

  if (element === null) {
    return { ok: false, problem: { kind: 'absent' } };
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(element.textContent ?? '');
  } catch (error) {
    return {
      ok: false,
      problem: { kind: 'unreadable', detail: String(error) },
    };
  }

  if (parsed === null) {
    return { ok: false, problem: { kind: 'template' } };
  }

  if (typeof parsed !== 'object') {
    return {
      ok: false,
      problem: { kind: 'unreadable', detail: `expected an object, found ${typeof parsed}` },
    };
  }

  const record = parsed as Record<string, unknown>;

  for (const [field, type] of Object.entries(SHAPE)) {
    if (typeof record[field] !== type) {
      return {
        ok: false,
        problem: {
          kind: 'unreadable',
          detail: `expected ${field} to be a ${type}, found ${typeof record[field]}`,
        },
      };
    }
  }

  if (record.schema_version !== SUBMISSION_SCHEMA_VERSION) {
    return {
      ok: false,
      problem: {
        kind: 'unreadable',
        detail:
          `expected schema version ${SUBMISSION_SCHEMA_VERSION}, ` +
          `found ${String(record.schema_version)}`,
      },
    };
  }

  return { ok: true, context: record as unknown as RemoteFormContext };
}

/** A sentence per problem, written for someone holding a file they were sent. */
export function problemMessage(problem: ContextProblem): string {
  switch (problem.kind) {
    case 'absent':
      return (
        'This file is not a meeting submission form. Ask the meeting host to '
        + 'send you the form they generated for you.'
      );
    case 'template':
      return (
        'This is the blank form template, not a form generated for you. Ask the '
        + 'meeting host to send you your own copy.'
      );
    case 'unreadable':
      return (
        'This form could not be read, so it may have been damaged on the way to '
        + 'you. Ask the meeting host to send it again.'
      );
  }
}
