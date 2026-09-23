import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SUBMISSION_CONTEXT_ELEMENT_ID, SUBMISSION_SCHEMA_VERSION } from '@lan-meeting/contracts';
import type { RemoteFormContext, RemoteSubmissionV1 } from '@lan-meeting/contracts';

import { renderApp } from '../src/app';
import { readContext } from '../src/context';
import {
  buildSubmission,
  serializeSubmission,
  submissionFilename,
} from '../src/export';

/**
 * The offline form, driven through a real DOM.
 *
 * `happy-dom` rather than a stand-in, because what is under test is largely
 * about the DOM: that identity is rendered as text rather than as fields, that
 * the note is prefilled, and that nothing reaches the page through an HTML
 * sink. Asserting that against a hand-written fake would be asserting it
 * against the fake.
 *
 * These live in `tests/` rather than beside the sources on purpose.
 * `apps/remote-form/src` is scanned by a boundary guard for absolute URLs and
 * network APIs, because that directory is what gets bundled into a file a
 * participant opens offline - and a test naturally mentions both.
 */

const CONTEXT: RemoteFormContext = {
  schema_version: SUBMISSION_SCHEMA_VERSION,
  submission_id: '0199c7e1-5f2a-7b3c-8d4e-5f6a7b8c9d0e' as RemoteFormContext['submission_id'],
  meeting_id: '0199c7e1-1111-7222-8333-444455556666' as RemoteFormContext['meeting_id'],
  meeting_title: 'Weekly Coordination',
  meeting_date: '2026-09-20' as RemoteFormContext['meeting_date'],
  meeting_timezone: 'Asia/Makassar' as RemoteFormContext['meeting_timezone'],
  participant_id: '0199c7e1-aaaa-7bbb-8ccc-ddddeeeeffff' as RemoteFormContext['participant_id'],
  participant_name: 'Budi Santoso',
  source_version: 3,
  existing_content: '## What I noted\n\n- First point',
  generated_at: '2026-09-23T01:00:00.000Z' as RemoteFormContext['generated_at'],
};

/** Put a context into the document the way the Host's generator does. */
function bakeIn(payload: string): void {
  document.body.replaceChildren();

  const island = document.createElement('script');
  island.type = 'application/json';
  island.id = SUBMISSION_CONTEXT_ELEMENT_ID;
  island.textContent = payload;

  const root = document.createElement('div');
  root.id = 'root';

  document.body.append(island, root);
}

function root(): HTMLElement {
  const node = document.getElementById('root');
  if (node === null) {
    throw new Error('no root');
  }
  return node;
}

function textarea(): HTMLTextAreaElement {
  const node = root().querySelector('textarea');
  if (node === null) {
    throw new Error('no textarea');
  }
  return node;
}

function exportButton(): HTMLButtonElement {
  const node = [...root().querySelectorAll('button')].find(
    (button) => button.textContent === 'Export submission',
  );
  if (node === undefined) {
    throw new Error('no export button');
  }
  return node;
}

beforeEach(() => {
  document.body.replaceChildren();
  vi.useRealTimers();
});

describe('reading this file\'s own context', () => {
  it('parses a generated form', () => {
    bakeIn(JSON.stringify(CONTEXT));
    const result = readContext();

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.context).toEqual(CONTEXT);
    }
  });

  it('recognises the unfilled template', () => {
    bakeIn('null');
    const result = readContext();

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.problem.kind).toBe('template');
    }
  });

  it('recognises a file that is not a form at all', () => {
    document.body.replaceChildren();
    const result = readContext();

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.problem.kind).toBe('absent');
    }
  });

  it('refuses a damaged payload rather than rendering half a form', () => {
    for (const payload of ['{ not json', '"a string"', '42', '[]']) {
      bakeIn(payload);
      const result = readContext();
      expect(result.ok, payload).toBe(false);
    }
  });

  it('refuses a missing or wrongly typed field', () => {
    const { participant_name: _omitted, ...missing } = CONTEXT;
    bakeIn(JSON.stringify(missing));
    expect(readContext().ok).toBe(false);

    bakeIn(JSON.stringify({ ...CONTEXT, source_version: 'three' }));
    expect(readContext().ok).toBe(false);
  });

  it('refuses a schema version this build does not know', () => {
    bakeIn(JSON.stringify({ ...CONTEXT, schema_version: 2 }));
    const result = readContext();

    expect(result.ok).toBe(false);
    if (!result.ok && result.problem.kind === 'unreadable') {
      expect(result.problem.detail).toContain('schema version');
    }
  });

  it('reads escaped markup back exactly as the Host put it in', () => {
    // The Host escapes every `<` as `<`, which `JSON.parse` decodes. What
    // the participant sees must be what the Host wrote.
    const hostile: RemoteFormContext = {
      ...CONTEXT,
      meeting_title: '</script><img src=x onerror=alert(1)>',
      existing_content: 'a < b and   a separator',
    };
    bakeIn(JSON.stringify(hostile).replace(/</g, '\\u003c'));

    const result = readContext();
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.context).toEqual(hostile);
    }
  });
});

describe('the rendered form', () => {
  it('prefills the note the Host baked in', () => {
    bakeIn(JSON.stringify(CONTEXT));
    renderApp(root());

    expect(textarea().value).toBe('## What I noted\n\n- First point');
  });

  it('opens empty when the participant has written nothing', () => {
    bakeIn(JSON.stringify({ ...CONTEXT, existing_content: '', source_version: 0 }));
    renderApp(root());

    expect(textarea().value).toBe('');
    // An untouched empty note is not shown as an error; it is a blank page.
    const notice = root().querySelector('.notice-validation') as HTMLElement | null;
    expect(notice?.hidden ?? true).toBe(true);
  });

  it('shows identity as text, with nothing to edit it with', () => {
    bakeIn(JSON.stringify(CONTEXT));
    renderApp(root());

    const shell = root().textContent ?? '';
    expect(shell).toContain('Budi Santoso');
    expect(shell).toContain('Weekly Coordination');
    // The date is never shown without its zone (PRD section 25.2).
    expect(shell).toContain('2026-09-20 (Asia/Makassar)');

    // One editable control before exporting: the note. No inputs, no selects.
    expect(root().querySelectorAll('input')).toHaveLength(0);
    expect(root().querySelectorAll('select')).toHaveLength(0);
    expect(root().querySelectorAll('textarea')).toHaveLength(1);
  });

  it('renders hostile text as text, never as markup', () => {
    bakeIn(
      JSON.stringify({
        ...CONTEXT,
        meeting_title: '<img src=x onerror=alert(1)>',
        participant_name: '<b>not bold</b>',
      }),
    );
    renderApp(root());

    expect(root().querySelector('img')).toBeNull();
    expect(root().querySelector('b')).toBeNull();
    expect(root().textContent).toContain('<img src=x onerror=alert(1)>');
  });

  it('says why a file that is not a form cannot be filled in', () => {
    bakeIn('null');
    renderApp(root());

    expect(root().textContent).toContain('blank form template');
    expect(root().querySelectorAll('textarea')).toHaveLength(0);
  });
});

describe('validation before export', () => {
  it('stays quiet until the participant engages with it', () => {
    bakeIn(JSON.stringify({ ...CONTEXT, existing_content: '', source_version: 0 }));
    renderApp(root());

    const notice = root().querySelector('.notice-validation') as HTMLElement;
    expect(notice.hidden).toBe(true);

    exportButton().click();
    expect(notice.hidden).toBe(false);
    expect(notice.textContent).toContain('needs some content');
  });

  it('refuses content the backend would refuse', () => {
    bakeIn(JSON.stringify(CONTEXT));
    renderApp(root());

    const field = textarea();
    field.value = 'a note with <div>raw html</div> in it';
    field.dispatchEvent(new Event('input'));

    const notice = root().querySelector('.notice-validation') as HTMLElement;
    expect(notice.hidden).toBe(false);
    expect(notice.textContent).toContain('HTML is not allowed');
  });

  it('is quiet again once the content is acceptable', () => {
    bakeIn(JSON.stringify(CONTEXT));
    renderApp(root());

    const field = textarea();
    field.value = '';
    field.dispatchEvent(new Event('input'));
    const notice = root().querySelector('.notice-validation') as HTMLElement;
    expect(notice.hidden).toBe(false);

    field.value = '## Fine now';
    field.dispatchEvent(new Event('input'));
    expect(notice.hidden).toBe(true);
  });
});

describe('the exported submission', () => {
  it('carries the identity the Host baked in, not anything the form invented', () => {
    const submission = buildSubmission(CONTEXT, '## Written offline');

    expect(submission.schema_version).toBe(SUBMISSION_SCHEMA_VERSION);
    expect(submission.submission_id).toBe(CONTEXT.submission_id);
    expect(submission.meeting_id).toBe(CONTEXT.meeting_id);
    expect(submission.participant_id).toBe(CONTEXT.participant_id);
    expect(submission.participant_name).toBe(CONTEXT.participant_name);
    expect(submission.source_version).toBe(CONTEXT.source_version);
    expect(submission.generated_at).toBe(CONTEXT.generated_at);
    expect(submission.note).toBe('## Written offline');
  });

  it('has exactly the fields the schema names, and no links array', () => {
    const submission = buildSubmission(CONTEXT, 'note');

    expect(Object.keys(submission).sort()).toEqual(
      [
        'generated_at',
        'meeting_id',
        'note',
        'participant_id',
        'participant_name',
        'schema_version',
        'source_version',
        'submission_id',
        'submitted_at',
      ].sort(),
    );
    expect('links' in submission).toBe(false);
  });

  it('stamps submitted_at at export time', () => {
    const at = new Date('2026-10-01T07:08:09.000Z');
    const submission = buildSubmission(CONTEXT, 'note', at);

    expect(submission.submitted_at).toBe('2026-10-01T07:08:09.000Z');
    // And it is the participant's clock, not the Host's.
    expect(submission.submitted_at).not.toBe(submission.generated_at);
  });

  it('serialises as readable JSON that parses back', () => {
    const submission = buildSubmission(CONTEXT, '## Written offline');
    const text = serializeSubmission(submission);

    expect(text.endsWith('\n')).toBe(true);
    expect(JSON.parse(text) as RemoteSubmissionV1).toEqual(submission);
  });

  it('suggests a filename that is a convenience, not an identity', () => {
    const name = submissionFilename(CONTEXT);

    expect(name).toBe('Weekly-Coordination-Budi-Santoso-submission.json');
    // A hostile title cannot become a path.
    expect(submissionFilename({ ...CONTEXT, meeting_title: '../../etc/passwd' })).not.toContain(
      '..',
    );
    expect(submissionFilename({ ...CONTEXT, meeting_title: '', participant_name: '' })).toBe(
      'meeting-participant-submission.json',
    );
  });
});

describe('exporting from the page', () => {
  /** Capture what the form would have handed to the browser. */
  function captureDownload(): { readonly text: () => string } {
    const captured: string[] = [];

    vi.spyOn(URL, 'createObjectURL').mockImplementation((blob: Blob | MediaSource) => {
      // `Blob.text()` is async; the constructor argument is what matters and is
      // captured by the stub below instead.
      void blob;
      return 'blob:captured';
    });
    vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined);

    const OriginalBlob = globalThis.Blob;
    vi.stubGlobal(
      'Blob',
      class extends OriginalBlob {
        constructor(parts: BlobPart[], options?: BlobPropertyBag) {
          super(parts, options);
          captured.push(parts.map(String).join(''));
        }
      },
    );

    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);

    return { text: () => captured.join('') };
  }

  it('produces the submission and shows it for copying', () => {
    bakeIn(JSON.stringify(CONTEXT));
    renderApp(root());

    const download = captureDownload();

    const field = textarea();
    field.value = '## Written offline\n\n- A point';
    field.dispatchEvent(new Event('input'));
    exportButton().click();

    const submission = JSON.parse(download.text()) as RemoteSubmissionV1;
    expect(submission.note).toBe('## Written offline\n\n- A point');
    expect(submission.submission_id).toBe(CONTEXT.submission_id);

    // The copyable fallback is always shown, because a download from `file://`
    // can be declined silently.
    const box = root().querySelector('.submission-text') as HTMLTextAreaElement | null;
    expect(box).not.toBeNull();
    expect(box?.readOnly).toBe(true);
    expect(JSON.parse(box?.value ?? '') as RemoteSubmissionV1).toEqual(submission);

    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('exports nothing when the note would be refused', () => {
    bakeIn(JSON.stringify({ ...CONTEXT, existing_content: '', source_version: 0 }));
    renderApp(root());

    const download = captureDownload();
    exportButton().click();

    expect(download.text()).toBe('');
    expect(root().querySelector('.submission-text')).toBeNull();

    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });
});
