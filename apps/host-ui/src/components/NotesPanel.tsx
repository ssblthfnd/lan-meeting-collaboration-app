import { useCallback, useEffect, useState } from 'react';
import { byteLength, MAX_NOTE_BYTES, validateNoteContent } from '@lan-meeting/editor';
import type {
  DomainEventKind,
  HostError,
  MeetingDetail,
  MeetingId,
  NoteDetail,
  ParticipantId,
} from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { useDomainEvents } from '../hooks/useDomainEvents';
import { useQuery } from '../hooks/useQuery';
import { ErrorNotice } from './ErrorNotice';
import { Markdown } from './Markdown';
import { NoteHistory } from './NoteHistory';

/**
 * The Host's view of participant notes.
 *
 * A roster on the left, one note on the right: read it, edit it, or look
 * through its history. The Host may write any participant's note while the
 * meeting is not locked (PRD section 15).
 *
 * # Nothing here is authority
 *
 * The editor validates content as it is typed, but that is a courtesy. Whether
 * a note is stored is decided in Rust, inside the mutating transaction, and
 * the same is true of the lock: hiding the Edit button on a locked meeting is
 * presentation, and a stale window that calls the command anyway is refused
 * identically (architecture rules section 15).
 *
 * # Content is hostile input, here too
 *
 * A note can have been written by a participant on the LAN. It is rendered
 * through the shared editor, which builds DOM nodes rather than markup, so
 * there is no HTML sink in this component and no sanitiser to forget
 * (PRD 13.3, ADR-0019).
 *
 * # Drafts are never overwritten
 *
 * `note.changed` can arrive while the Host is mid-sentence - a participant
 * editing their own note, or the same Host in another window. When the editor
 * is clean, the note is reloaded silently. When it is dirty, the draft is left
 * exactly as it is and the Host is told, with the choice theirs to make.
 * Saving anyway is last-write-wins (D8), and what was overwritten is still in
 * history, which is what history is for.
 */

/** The events that change what this panel shows. */
const WATCHED: readonly DomainEventKind[] = ['note.changed', 'participant.removed'];

/** What the Host is doing with the selected note. */
type Mode =
  | { readonly kind: 'reading' }
  | { readonly kind: 'editing'; readonly draft: string; readonly base: number | null };

export function NotesPanel({ meeting }: { readonly meeting: MeetingDetail }) {
  const meetingId: MeetingId = meeting.id;
  const editable = meeting.status !== 'LOCKED';

  const overview = useQuery(() => hostApi.listNotesOverview(meetingId), [meetingId]);
  const [selected, setSelected] = useState<ParticipantId | null>(null);
  const [mode, setMode] = useState<Mode>({ kind: 'reading' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<HostError | null>(null);
  /** Set when the note changed underneath an unsaved draft. */
  const [changedUnderneath, setChangedUnderneath] = useState(false);

  const roster = overview.data ?? [];

  // Select the first participant once the roster arrives, so the panel does
  // not open on an empty right-hand side.
  useEffect(() => {
    if (selected === null && roster.length > 0) {
      setSelected(roster[0]?.participant_id ?? null);
    }
  }, [roster, selected]);

  const note = useQuery(
    () =>
      selected === null
        ? Promise.resolve(null)
        : hostApi.getParticipantNote(meetingId, selected),
    [meetingId, selected],
  );

  const reload = useCallback(() => {
    note.reload();
    overview.reload();
    // Remount the history list so a new version appears in it.
    setHistoryKey((key) => key + 1);
  }, [note, overview]);

  const [historyKey, setHistoryKey] = useState(0);

  useDomainEvents(meetingId, WATCHED, (event) => {
    // Another participant's note changing does not affect what is on screen.
    if (event.type === 'note.changed' && event.participant_id !== selected) {
      overview.reload();
      return;
    }
    if (mode.kind === 'editing') {
      // A draft is never clobbered. The Host decides.
      setChangedUnderneath(true);
      overview.reload();
      return;
    }
    reload();
  });

  function startEditing(current: NoteDetail | null) {
    setError(null);
    setChangedUnderneath(false);
    setMode({
      kind: 'editing',
      draft: current?.content ?? '',
      base: current?.version ?? null,
    });
  }

  function cancelEditing() {
    setError(null);
    setChangedUnderneath(false);
    setMode({ kind: 'reading' });
  }

  function discardAndReload() {
    setChangedUnderneath(false);
    setMode({ kind: 'reading' });
    reload();
  }

  async function save() {
    if (mode.kind !== 'editing' || selected === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await hostApi.writeParticipantNote(meetingId, selected, mode.draft);
      setMode({ kind: 'reading' });
      setChangedUnderneath(false);
      // Re-read rather than patching local state: the backend decided the
      // version, and it is authoritative about what the note now says.
      reload();
    } catch (rejection) {
      setError(rejection as HostError);
    } finally {
      setBusy(false);
    }
  }

  const current = note.data ?? null;
  const selectedRow = roster.find((row) => row.participant_id === selected);

  return (
    <section className="panel">
      <header className="panel-head">
        <h3>Notes</h3>
        <span className="meta">
          {roster.filter((row) => row.note_id !== null).length} of {roster.length} written
        </span>
      </header>

      {!editable && (
        <p className="notice notice-lifecycle">
          This meeting is <code>LOCKED</code>. Notes can be read and their history
          reviewed, but nothing can be changed.
        </p>
      )}

      {overview.error && <ErrorNotice error={overview.error} />}

      {roster.length === 0 ? (
        <p className="empty">No participants yet. Add someone before writing notes.</p>
      ) : (
        <div className="notes-layout">
          <ul className="rows notes-roster">
            {roster.map((row) => (
              <li key={row.participant_id}>
                <button
                  type="button"
                  className={
                    row.participant_id === selected ? 'note-pick active' : 'note-pick'
                  }
                  onClick={() => {
                    if (mode.kind === 'editing' && row.participant_id !== selected) {
                      // Switching away would lose the draft silently.
                      return;
                    }
                    setSelected(row.participant_id);
                  }}
                  disabled={mode.kind === 'editing' && row.participant_id !== selected}
                >
                  <strong>{row.name}</strong>
                  <span className="card-meta">
                    {row.note_id === null
                      ? 'No note yet'
                      : `Version ${row.version ?? 1} · ${row.updated_at ?? ''}`}
                  </span>
                </button>
              </li>
            ))}
          </ul>

          <div className="notes-detail">
            {mode.kind === 'editing' && (
              <p className="meta">
                Editing {selectedRow?.name ?? 'this note'}. Other participants cannot be
                selected until you save or cancel.
              </p>
            )}

            {note.error && <ErrorNotice error={note.error} />}
            {error && <ErrorNotice error={error} />}

            {changedUnderneath && (
              <div className="notice notice-conflict" role="alert">
                <p>
                  This note was changed elsewhere while you were editing. Your unsaved
                  draft has been left exactly as it is.
                </p>
                <div className="actions">
                  <button type="button" onClick={() => setChangedUnderneath(false)}>
                    Keep editing
                  </button>
                  <button type="button" className="danger" onClick={discardAndReload}>
                    Discard mine and reload
                  </button>
                </div>
                <p className="meta">
                  Saving anyway keeps your text and records it as a new version. Nothing
                  is lost either way: every earlier version stays in the history below.
                </p>
              </div>
            )}

            {note.loading && current === null && mode.kind === 'reading' && (
              <p className="meta">Loading…</p>
            )}

            {mode.kind === 'reading' ? (
              <NoteReader
                note={current}
                editable={editable}
                onEdit={() => startEditing(current)}
              />
            ) : (
              <NoteEditor
                draft={mode.draft}
                base={mode.base}
                busy={busy}
                onChange={(draft) => setMode({ ...mode, draft })}
                onSave={() => void save()}
                onCancel={cancelEditing}
              />
            )}

            {selected !== null && (
              <NoteHistory
                key={`${selected}-${historyKey}`}
                meetingId={meetingId}
                participantId={selected}
              />
            )}
          </div>
        </div>
      )}
    </section>
  );
}

/** The note as it stands. */
function NoteReader({
  note,
  editable,
  onEdit,
}: {
  readonly note: NoteDetail | null;
  readonly editable: boolean;
  readonly onEdit: () => void;
}) {
  return (
    <section className="panel note-panel">
      <header className="panel-head">
        <h4>{note === null ? 'No note yet' : `Current note · version ${note.version}`}</h4>
        {editable && (
          <button type="button" onClick={onEdit}>
            {note === null ? 'Write a note' : 'Edit'}
          </button>
        )}
      </header>

      {note === null ? (
        <p className="empty">
          This participant has not written anything yet.
          {editable ? ' You can write it for them.' : ''}
        </p>
      ) : (
        <>
          <p className="meta">
            Last changed by {note.last_author_type} · {note.updated_at}
          </p>
          <Markdown content={note.content} />
        </>
      )}
    </section>
  );
}

/**
 * The editing surface: the Markdown itself, and a live preview beside it.
 *
 * The textarea holds the raw Markdown, which *is* the note. There is no
 * document model in between, so nothing rewrites a Host's formatting between
 * typing it and storing it (ADR-0019).
 */
function NoteEditor({
  draft,
  base,
  busy,
  onChange,
  onSave,
  onCancel,
}: {
  readonly draft: string;
  readonly base: number | null;
  readonly busy: boolean;
  readonly onChange: (draft: string) => void;
  readonly onSave: () => void;
  readonly onCancel: () => void;
}) {
  const bytes = byteLength(draft);
  // The backend runs these same rules and is the one that decides. This is so
  // the Host is told while typing rather than after pressing save.
  const problem = validateNoteContent(draft);

  return (
    <section className="panel note-panel">
      <header className="panel-head">
        <h4>{base === null ? 'Writing a new note' : `Editing version ${base}`}</h4>
        <span className={bytes > MAX_NOTE_BYTES ? 'meta over-limit' : 'meta'}>
          {draft.length} characters · {bytes} of {MAX_NOTE_BYTES} bytes
        </span>
      </header>

      <div className="note-edit">
        <label className="note-source">
          <span>Markdown</span>
          <textarea
            value={draft}
            rows={18}
            spellCheck
            onChange={(event) => onChange(event.target.value)}
            placeholder={'## Agenda\n\n- Budget\n- Venue\n\nSee [the brief](https://…)'}
          />
        </label>

        <div className="note-preview">
          <span className="meta">Preview</span>
          <Markdown content={draft} />
        </div>
      </div>

      {problem !== null && (
        <p className="notice notice-validation" role="alert">
          {problemMessage(problem.reason)} Expected {problem.expected}; found{' '}
          {problem.detected}.
        </p>
      )}

      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={busy || problem !== null}
          onClick={onSave}
        >
          {busy ? 'Saving…' : 'Save note'}
        </button>
        <button type="button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
        <span className="meta">
          Saving records a new version. Earlier versions are kept and are never
          changed.
        </span>
      </div>
    </section>
  );
}

/** A sentence per refusal, so the count and the scheme are not the whole message. */
function problemMessage(reason: string): string {
  switch (reason) {
    case 'empty':
      return 'A note needs some content.';
    case 'too_large':
      return 'This note is too long to store.';
    case 'raw_html':
      return 'Raw HTML is not allowed in a note.';
    case 'link_scheme':
      return 'Links may only use http, https or mailto.';
    case 'control_character':
      return 'This note contains a character that cannot be stored.';
    default:
      return 'This note cannot be saved yet.';
  }
}
