import { useEffect, useState } from 'react';
import { byteLength, MAX_NOTE_BYTES, validateNoteContent } from '@lan-meeting/editor';
import type { LanError, LanMeetingView, LanNoteView } from '@lan-meeting/contracts';

import { Markdown } from './Markdown';
import { Notice } from './Notice';

/**
 * The participant's own note.
 *
 * One note per participant per meeting (ADR-0003), and this is theirs. A
 * participant may create and edit it while the meeting is open (PRD section
 * 15); once the meeting is locked the controls go away and the note stays
 * readable.
 *
 * # Nothing here is authority
 *
 * The editor checks content as it is typed, but that is a courtesy so the
 * participant is told while writing rather than on save. Whether a note is
 * stored is decided in Rust inside the mutating transaction, and so is the
 * lock: hiding the Edit button is presentation, and a stale screen that calls
 * the API anyway is refused identically (architecture rules section 15).
 *
 * # Drafts are never overwritten
 *
 * `note.changed` can arrive mid-sentence - the Host editing this note, or the
 * same person in another tab. The parent decides when to reload; this component
 * is told whether the note changed underneath an unsaved draft, and offers the
 * choice rather than making it. Saving anyway is last-write-wins, and what was
 * replaced is still in the note's history (ADR-0020).
 */
export function NotePanel({
  meeting,
  note,
  loading,
  changedElsewhere,
  onSave,
  onReload,
  onDismissChange,
  onEditingChange,
}: {
  readonly meeting: LanMeetingView;
  readonly note: LanNoteView | null;
  readonly loading: boolean;
  /** True when the note changed on the host while a draft is unsaved. */
  readonly changedElsewhere: boolean;
  readonly onSave: (content: string) => Promise<LanError | null>;
  readonly onReload: () => void;
  readonly onDismissChange: () => void;
  /** Told whenever the editor opens or closes, so a draft is never clobbered. */
  readonly onEditingChange: (editing: boolean) => void;
}) {
  const editable = meeting.status !== 'LOCKED';
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<LanError | null>(null);

  const editing = draft !== null;

  // The parent decides what to do when `note.changed` arrives, and it needs to
  // know whether there is a draft on screen to protect. Reported rather than
  // inferred, so the two cannot disagree.
  useEffect(() => {
    onEditingChange(editing);
  }, [editing, onEditingChange]);

  function startEditing() {
    setError(null);
    setDraft(note?.content ?? '');
  }

  function cancelEditing() {
    setError(null);
    setDraft(null);
    onDismissChange();
  }

  function discardAndReload() {
    setDraft(null);
    setError(null);
    onDismissChange();
    onReload();
  }

  async function save() {
    if (draft === null) {
      return;
    }
    setBusy(true);
    setError(null);
    const rejection = await onSave(draft);
    setBusy(false);

    if (rejection === null) {
      setDraft(null);
      onDismissChange();
    } else {
      setError(rejection);
    }
  }

  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Your note</h2>
        {note !== null && <span className="meta">Version {note.version}</span>}
      </header>

      {!editable && (
        <p className="notice notice-lifecycle">
          This meeting is locked, so your note can no longer be changed. It is
          still here to read.
        </p>
      )}

      {error !== null && <Notice error={error} />}

      {changedElsewhere && editing && (
        <div className="notice notice-conflict" role="alert">
          <p>
            Your note was changed elsewhere while you were writing. Your unsaved
            text has been left exactly as it is.
          </p>
          <div className="actions">
            <button type="button" onClick={onDismissChange}>
              Keep editing
            </button>
            <button type="button" className="danger" onClick={discardAndReload}>
              Discard mine and reload
            </button>
          </div>
          <p className="meta">
            Saving anyway keeps your text and records it as a new version.
            Nothing is lost either way: every earlier version is kept.
          </p>
        </div>
      )}

      {loading && note === null && !editing && <p className="meta">Loading…</p>}

      {editing ? (
        <NoteEditor
          draft={draft}
          busy={busy}
          onChange={setDraft}
          onSave={() => void save()}
          onCancel={cancelEditing}
        />
      ) : (
        <>
          {note === null ? (
            <p className="empty">
              You have not written anything yet.
              {editable ? ' Write your note for this meeting below.' : ''}
            </p>
          ) : (
            <>
              {note.last_author_type === 'HOST' && (
                <p className="meta">The host last changed this note.</p>
              )}
              <Markdown content={note.content} />
              <p className="meta">Last saved {note.updated_at}</p>
            </>
          )}

          {editable && (
            <div className="actions">
              <button type="button" className="primary" onClick={startEditing}>
                {note === null ? 'Write your note' : 'Edit'}
              </button>
            </div>
          )}
        </>
      )}
    </section>
  );
}

/**
 * The editing surface: the Markdown itself, with a live preview below it.
 *
 * The textarea holds the raw Markdown, which *is* the note. There is no
 * document model in between, so nothing rewrites what was typed between typing
 * it and storing it (ADR-0019).
 */
function NoteEditor({
  draft,
  busy,
  onChange,
  onSave,
  onCancel,
}: {
  readonly draft: string;
  readonly busy: boolean;
  readonly onChange: (draft: string) => void;
  readonly onSave: () => void;
  readonly onCancel: () => void;
}) {
  const bytes = byteLength(draft);
  // The backend runs these same rules and is the one that decides. This is so
  // the participant is told while typing rather than after pressing save.
  const problem = validateNoteContent(draft);
  /**
   * Whether the rules have anything to say *yet*.
   *
   * An editor just opened on a note nobody has written is empty, and
   * `validateNoteContent('')` correctly calls that unsavable - but greeting
   * someone with a refusal before they have typed a character is a statement
   * about the blank page, not about their note. So the message appears from
   * the first keystroke, or from the first attempt to save, whichever is first.
   *
   * Only *showing* the message changes. Nothing is relaxed: an empty draft is
   * still never sent, and the domain re-runs every rule inside the mutating
   * transaction regardless (architecture rules section 15).
   */
  const [engaged, setEngaged] = useState(false);
  const showProblem = problem !== null && engaged;

  function attemptSave() {
    // Pressing save is engaging with the rules, so from here on they are shown.
    setEngaged(true);
    if (problem === null) {
      onSave();
    }
  }

  return (
    <div className="note-edit">
      <label className="note-source">
        <span>Your note</span>
        <textarea
          value={draft}
          rows={12}
          spellCheck
          autoFocus
          onChange={(event) => {
            setEngaged(true);
            onChange(event.target.value);
          }}
          placeholder={'## What I noted\n\n- First point\n- Second point'}
        />
      </label>

      <p className={bytes > MAX_NOTE_BYTES ? 'meta over-limit' : 'meta'}>
        {draft.length} characters · {bytes} of {MAX_NOTE_BYTES} bytes
      </p>

      {showProblem && (
        <p className="notice notice-validation" role="alert">
          {problemMessage(problem.reason)}
        </p>
      )}

      <div className="note-preview">
        <span className="meta">Preview</span>
        <Markdown content={draft} />
      </div>

      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={busy || showProblem}
          onClick={attemptSave}
        >
          {busy ? 'Saving…' : 'Save note'}
        </button>
        <button type="button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

/** A sentence per refusal, written for someone in a meeting room. */
function problemMessage(reason: string): string {
  switch (reason) {
    case 'empty':
      return 'A note needs some content before it can be saved.';
    case 'too_large':
      return 'This note is too long to save. Shorten it and try again.';
    case 'raw_html':
      return 'HTML is not allowed in a note. Write it as plain text or as code.';
    case 'link_scheme':
      return 'Links may only point at http, https or an email address.';
    case 'control_character':
      return 'This note contains a character that cannot be saved.';
    default:
      return 'This note cannot be saved yet.';
  }
}
