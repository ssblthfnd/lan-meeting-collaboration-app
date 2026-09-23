import {
  byteLength,
  MAX_NOTE_BYTES,
  renderMarkdown,
  validateNoteContent,
} from '@lan-meeting/editor';
import type { RemoteFormContext } from '@lan-meeting/contracts';

import {
  buildSubmission,
  offerDownload,
  serializeSubmission,
  submissionFilename,
} from './export';

/**
 * The form a remote participant fills in.
 *
 * Plain DOM, no framework (ADR-0009), and no network of any kind. Text reaches
 * the page through `textContent` and through the shared renderer, which builds
 * DOM nodes rather than markup - so there is no HTML sink in this bundle and no
 * sanitiser to forget.
 *
 * # What is fixed, and what is theirs
 *
 * Identity and meeting metadata are **read-only** (PRD section 10): the Host
 * chose them, and the form renders them as text rather than as fields. The note
 * is the one thing a participant may change, prefilled with whatever they had
 * already written (ADR-0021 decision 4).
 *
 * Nothing here is authority. The identifiers travel back so the Host can
 * *resolve* them, and the Host's database and confirmation decide whether the
 * submission is imported at all. Validation below is a courtesy so a
 * participant is told while writing rather than after sending a file to someone.
 * `app-core::note` runs the same rules and is what decides (architecture rules
 * section 3).
 */

/** Create an element, optionally with a class and plain text. */
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

/** One read-only fact about the meeting or the participant. */
function fact(label: string, value: string): HTMLDivElement {
  const row = el('div', 'fact');
  row.append(el('span', 'fact-label', label), el('span', 'fact-value', value));
  return row;
}

/** A sentence per refusal, written for someone in a meeting room. */
function problemMessage(reason: string): string {
  switch (reason) {
    case 'empty':
      return 'A note needs some content before it can be exported.';
    case 'too_large':
      return 'This note is too long to send. Shorten it and try again.';
    case 'raw_html':
      return 'HTML is not allowed in a note. Write it as plain text or as code.';
    case 'link_scheme':
      return 'Links may only point at http, https or an email address.';
    case 'control_character':
      return 'This note contains a character that cannot be saved.';
    default:
      return 'This note cannot be exported yet.';
  }
}

/** Render the filled-in form for `context` into `container`. */
export function renderForm(container: HTMLElement, context: RemoteFormContext): void {
  const shell = el('main', 'shell');

  shell.append(
    el('h1', null, 'Meeting submission'),
    el('p', 'subtitle', context.meeting_title),
  );

  /* ---------------------------------------------------------------------
   * Who and what this form is for. Read-only, and stated rather than typed.
   * ------------------------------------------------------------------- */
  const identity = el('section', 'card');
  identity.append(
    el('h2', null, 'This form is for you'),
    fact('Participant', context.participant_name),
    // The date means nothing without the zone, so the two are never separated
    // (PRD section 25.2, ADR-0005).
    fact('Meeting date', `${context.meeting_date} (${context.meeting_timezone})`),
    el(
      'p',
      'meta',
      'These details were filled in by the meeting host and cannot be changed here.',
    ),
  );
  shell.append(identity);

  /* ---------------------------------------------------------------------
   * The note.
   * ------------------------------------------------------------------- */
  const editor = el('section', 'card');
  editor.append(el('h2', null, 'Your note'));

  if (context.source_version > 0) {
    editor.append(
      el(
        'p',
        'meta',
        `Your note as the host had it (version ${context.source_version}) is already `
          + 'below. Edit it, or replace it entirely.',
      ),
    );
  }

  const label = el('label', 'note-source');
  label.append(el('span', null, 'Write your note in Markdown'));

  const textarea = document.createElement('textarea');
  textarea.rows = 14;
  textarea.spellcheck = true;
  textarea.value = context.existing_content;
  textarea.placeholder = '## What I noted\n\n- First point\n- Second point';
  label.append(textarea);
  editor.append(label);

  const counter = el('p', 'meta');
  editor.append(counter);

  const validation = el('p', 'notice notice-validation');
  validation.setAttribute('role', 'alert');
  validation.hidden = true;
  editor.append(validation);

  const previewHead = el('span', 'meta', 'Preview');
  const preview = el('div', 'note-preview');
  editor.append(previewHead, preview);

  /* ---------------------------------------------------------------------
   * Export. Nothing leaves this browser until the button below is pressed.
   * ------------------------------------------------------------------- */
  const actions = el('div', 'actions');
  const exportButton = el('button', 'primary', 'Export submission');
  exportButton.type = 'button';
  actions.append(exportButton);
  editor.append(actions);

  editor.append(
    el(
      'p',
      'meta',
      'Nothing is sent anywhere. Exporting saves a file on this device, which '
        + 'you then send back to the meeting host however you like.',
    ),
  );

  const result = el('section', 'card');
  result.hidden = true;
  editor.append(result);

  shell.append(editor);
  container.replaceChildren(shell);

  /* ---------------------------------------------------------------------
   * Behaviour.
   * ------------------------------------------------------------------- */

  /**
   * Whether the rules have anything to say *yet*.
   *
   * An untouched empty note is empty because nobody has typed, not because it
   * is wrong. The message appears from the first keystroke or the first export
   * attempt, whichever comes first; the rules themselves never relax, and an
   * empty note is never exported.
   */
  let engaged = context.existing_content.length > 0;

  function refresh(): void {
    const draft = textarea.value;
    const bytes = byteLength(draft);
    const problem = validateNoteContent(draft);

    counter.textContent = `${draft.length} characters · ${bytes} of ${MAX_NOTE_BYTES} bytes`;
    counter.className = bytes > MAX_NOTE_BYTES ? 'meta over-limit' : 'meta';

    const show = problem !== null && engaged;
    validation.hidden = !show;
    validation.textContent = show ? problemMessage(problem.reason) : '';

    // The shared renderer, which returns DOM nodes. No markup is ever assigned.
    preview.replaceChildren(renderMarkdown(draft));
  }

  textarea.addEventListener('input', () => {
    engaged = true;
    result.hidden = true;
    refresh();
  });

  exportButton.addEventListener('click', () => {
    engaged = true;
    refresh();

    if (validateNoteContent(textarea.value) !== null) {
      // Refused locally, so the participant is not left holding a file the Host
      // would have to reject. The backend would refuse it too.
      return;
    }

    const submission = buildSubmission(context, textarea.value);
    const text = serializeSubmission(submission);
    const filename = submissionFilename(context);
    const downloaded = offerDownload(text, filename);

    showResult(result, text, filename, downloaded);
  });

  refresh();
  textarea.focus();
}

/**
 * What the participant sees once they have exported.
 *
 * The copyable text is shown whether or not the download was offered. A
 * download from `file://` can be declined silently, and a participant who
 * cannot tell whether it worked should not have to guess - the text they need
 * is right there either way.
 */
function showResult(
  container: HTMLElement,
  text: string,
  filename: string,
  downloaded: boolean,
): void {
  const heading = el('h2', null, 'Your submission');

  const explanation = el(
    'p',
    null,
    downloaded
      ? `Saved as ${filename}. Send that file back to the meeting host. If your `
        + 'browser did not save it, copy the text below instead.'
      : `Your browser did not save a file. Copy the text below, save it as `
        + `${filename}, and send it to the meeting host.`,
  );

  const box = document.createElement('textarea');
  box.className = 'submission-text';
  box.rows = 10;
  box.readOnly = true;
  box.value = text;

  const copyAction = el('div', 'actions');
  const copyButton = el('button', null, 'Select all');
  copyButton.type = 'button';
  copyButton.addEventListener('click', () => {
    box.focus();
    box.select();
  });
  copyAction.append(copyButton);

  container.replaceChildren(heading, explanation, box, copyAction);
  container.hidden = false;
  container.scrollIntoView({ block: 'nearest' });
}
