import { MAX_LINKS_PER_NOTE, SUBMISSION_SCHEMA_VERSION } from '@lan-meeting/contracts';

/**
 * Remote submission form shell.
 *
 * Skeleton only. The immutable metadata block, the note fields, client-side
 * validation and the "Export Submission" action are Phase 1 work and are not
 * implemented yet.
 *
 * Built with plain DOM APIs and no framework, on purpose (ADR-0009). A
 * framework runtime ships bytes and string literals the offline guard cannot
 * vouch for - React's production build, for example, embeds an absolute
 * documentation URL in its error helper - and this is the one bundle whose
 * contract is that it carries no external dependency at all.
 *
 * Deliberately, no comment in this bundle spells out such a URL either: the
 * guard scans the built file, and a comment that survived an unminified build
 * would trip it for no reason.
 *
 * Invariants that already apply to anything added here:
 * - no network access of any kind; the file must work offline from file://
 * - identity metadata is read-only for the participant
 * - the submission leaves the browser only when the participant explicitly
 *   asks for it
 */

/**
 * Create an element, optionally with a class name and plain text.
 *
 * Text is always assigned through `textContent`. Nothing in this bundle
 * assigns `innerHTML`: meeting and participant content is hostile input here
 * too, and an HTML sink inside the offline form is a sanitiser bypass that no
 * backend check can catch (architecture rules 20 and 22).
 */
function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string | null,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);

  if (className !== null) {
    node.className = className;
  }

  if (text !== undefined) {
    node.textContent = text;
  }

  return node;
}

/** Render the form shell into `container`, replacing anything already there. */
export function renderApp(container: HTMLElement): void {
  const shell = el('main', 'shell');

  shell.append(
    el('h1', null, 'Meeting Submission Form'),
    el('p', 'subtitle', 'Remote participant - project skeleton'),
    el(
      'p',
      null,
      'This file works offline. Nothing is sent anywhere: a submission is ' +
        'produced only when you export it yourself.',
    ),
    el(
      'p',
      'meta',
      `Schema version ${SUBMISSION_SCHEMA_VERSION} - up to ${MAX_LINKS_PER_NOTE} links`,
    ),
  );

  container.replaceChildren(shell);
}
