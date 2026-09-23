import { problemMessage, readContext } from './context';
import { renderForm } from './form';

/**
 * Remote submission form shell.
 *
 * One HTML file that opens offline from `file://`. It has no server, no API, no
 * CDN and no credential; the Host generated it, sent it by whatever channel
 * they chose, and will read back whatever the participant exports
 * (PRD section 10, architecture rules sections 6 and 20).
 *
 * Built with plain DOM APIs and no framework, on purpose (ADR-0009). A
 * framework runtime ships bytes and string literals the offline guard cannot
 * vouch for, and this is the one bundle whose contract is that it carries no
 * external dependency at all.
 *
 * Deliberately, no comment in this bundle spells out an absolute URL either:
 * the guard scans the built file, and a comment that survived an unminified
 * build would trip it for no reason.
 *
 * Invariants that apply to anything added here:
 * - no network access of any kind; the file must work offline from `file://`
 * - identity metadata is read-only for the participant
 * - the submission leaves the browser only when the participant asks for it
 * - nothing assigns `innerHTML`; text reaches the page through `textContent`
 *   and through the shared renderer, which builds DOM nodes
 */

/** Create an element, optionally with a class name and plain text. */
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

/** Render the form, or say why this file cannot be filled in. */
export function renderApp(container: HTMLElement): void {
  const result = readContext();

  if (result.ok) {
    renderForm(container, result.context);
    return;
  }

  const shell = el('main', 'shell');
  shell.append(
    el('h1', null, 'Meeting submission'),
    el('p', 'notice notice-validation', problemMessage(result.problem)),
    el(
      'p',
      'meta',
      'This file works offline and sends nothing anywhere. It only becomes a '
        + 'form once a meeting host generates one for a named participant.',
    ),
  );

  container.replaceChildren(shell);
}
