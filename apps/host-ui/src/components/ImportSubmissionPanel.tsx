import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  HostError,
  ImportBlocker,
  MeetingDetail,
  RemoteSubmissionImported,
  RemoteSubmissionPreview,
} from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { ErrorNotice } from './ErrorNotice';
import { Markdown } from './Markdown';

/**
 * Reading a remote participant's submission back in (PRD sections 11 and 12).
 *
 * The other half of the remote flow: the Host generated a form, the participant
 * filled it in offline, and this is where the file they sent back becomes a
 * note. Nothing in this application receives it over a network - the Host has
 * the file, and drops it here or pastes it.
 *
 * # Preview, then confirm
 *
 * Two steps, and the separation is the point. The preview is read-only and
 * decides nothing; the confirmation re-runs every check against current state,
 * inside one transaction, and is what decides (ADR-0022 decision 4).
 *
 * Between them the meeting can be locked, the participant removed and the note
 * changed. A preview that said *eligible* is therefore a report and not a
 * promise, and this component never treats it as one: what it disables is a
 * button, and the backend refuses regardless.
 *
 * # Identity is the database's
 *
 * A submission carries a participant id and a name. The id is resolved against
 * this meeting's roster and the **roster's own name** is what is shown as
 * authoritative; the name the file claims appears beside it so a mismatch is
 * visible, and it is stored nowhere (ADR-0022 decision 2).
 *
 * # Content is hostile input
 *
 * The submitted note came from outside. It is rendered through the shared
 * editor, which builds DOM nodes rather than markup, so there is no HTML sink
 * here and no sanitiser to forget.
 */

/** What the Host is shown for each way an import can be refused. */
const BLOCKER_MESSAGE: Record<ImportBlocker, string> = {
  meeting_mismatch:
    'This submission was made for a different meeting. Open that meeting and import it there.',
  meeting_locked:
    'This meeting is locked, so notes can no longer be changed. Nothing was imported.',
  participant_not_found:
    'No participant on this roster matches the submission. They may have been removed.',
  invalid_content: 'The note in this submission is not something the backend will store.',
  duplicate:
    'This exact submission has already been imported. Nothing was imported again.',
  modified_artifact:
    'This submission was already imported with different content, so the file has been '
    + 'altered since it was generated. Ask the participant for a freshly generated form.',
  cross_participant_artifact:
    'This form was generated for a different participant and has already been imported for '
    + 'them. Ask for a form generated for this participant.',
};

/** A sentence per content refusal, matching the backend's own reasons. */
const CONTENT_PROBLEM: Record<string, string> = {
  empty: 'The submitted note is empty.',
  too_large: 'The submitted note is longer than a note may be.',
  raw_html: 'The submitted note contains HTML, which is not allowed in a note.',
  link_scheme: 'A link in the submitted note points somewhere that is not allowed.',
  control_character: 'The submitted note contains a character that cannot be stored.',
};

type Stage =
  | { readonly kind: 'idle' }
  | { readonly kind: 'previewing' }
  | { readonly kind: 'preview'; readonly preview: RemoteSubmissionPreview }
  | { readonly kind: 'importing'; readonly preview: RemoteSubmissionPreview }
  | { readonly kind: 'done'; readonly imported: RemoteSubmissionImported };

export function ImportSubmissionPanel({
  meeting,
  onImported,
}: {
  readonly meeting: MeetingDetail;
  readonly onImported: () => void;
}) {
  const meetingId = meeting.id;
  const [stage, setStage] = useState<Stage>({ kind: 'idle' });
  const [error, setError] = useState<HostError | null>(null);
  const [pasted, setPasted] = useState('');
  const [dropNotice, setDropNotice] = useState<string | null>(null);

  // Held in a ref so a fresh closure on every render does not tear the
  // subscription down and build it again.
  const meetingRef = useRef(meetingId);
  meetingRef.current = meetingId;

  const preview = useCallback(async () => {
    setError(null);
    setStage({ kind: 'previewing' });
    try {
      setStage({
        kind: 'preview',
        preview: await hostApi.previewRemoteSubmission(meetingRef.current),
      });
    } catch (rejection) {
      setStage({ kind: 'idle' });
      setError(rejection as HostError);
    }
  }, []);

  // A file dropped on the window. The path stays in Rust; what arrives here is
  // the news that something is waiting (ADR-0022 decision 17).
  useEffect(() => {
    let cancelled = false;
    let stop: (() => void) | null = null;

    void hostApi
      .onRemoteSubmissionPending((event) => {
        if (event.error !== null) {
          setDropNotice(event.error);
          return;
        }
        setDropNotice(null);
        void preview();
      })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          stop = unlisten;
        }
      });

    return () => {
      cancelled = true;
      stop?.();
    };
  }, [preview]);

  async function usePasted() {
    setError(null);
    setDropNotice(null);
    try {
      await hostApi.remoteSubmissionFromText(pasted);
      await preview();
    } catch (rejection) {
      setError(rejection as HostError);
    }
  }

  async function confirm(current: RemoteSubmissionPreview) {
    setError(null);
    setStage({ kind: 'importing', preview: current });
    try {
      const imported = await hostApi.confirmRemoteSubmission(meetingId);
      setStage({ kind: 'done', imported });
      setPasted('');
      onImported();
    } catch (rejection) {
      // The artefact stays pending, so the Host can look at it again.
      setStage({ kind: 'preview', preview: current });
      setError(rejection as HostError);
    }
  }

  async function discard() {
    setError(null);
    setDropNotice(null);
    setPasted('');
    setStage({ kind: 'idle' });
    try {
      await hostApi.clearRemoteSubmission();
    } catch (rejection) {
      setError(rejection as HostError);
    }
  }

  return (
    <section className="panel">
      <header className="panel-head">
        <h3>Import a remote submission</h3>
      </header>

      <p className="meta">
        Drop the submission file a remote participant sent you onto this window,
        or paste its contents below. Nothing is imported until you confirm it.
      </p>

      {error !== null && <ErrorNotice error={error} />}

      {dropNotice !== null && (
        <p className="notice notice-validation" role="alert">
          {dropNotice}
        </p>
      )}

      {(stage.kind === 'idle' || stage.kind === 'previewing') && (
        <div className="import-paste">
          <label>
            <span>Or paste the submission JSON</span>
            <textarea
              rows={5}
              value={pasted}
              spellCheck={false}
              placeholder='{ "schema_version": 1, ... }'
              onChange={(event) => setPasted(event.target.value)}
            />
          </label>
          <div className="actions">
            <button
              type="button"
              className="primary"
              disabled={pasted.trim() === '' || stage.kind === 'previewing'}
              onClick={() => void usePasted()}
            >
              {stage.kind === 'previewing' ? 'Reading…' : 'Preview pasted submission'}
            </button>
          </div>
        </div>
      )}

      {(stage.kind === 'preview' || stage.kind === 'importing') && (
        <Preview
          preview={stage.preview}
          busy={stage.kind === 'importing'}
          onConfirm={() => void confirm(stage.preview)}
          onDiscard={() => void discard()}
        />
      )}

      {stage.kind === 'done' && (
        <div className="import-result">
          <p className="notice notice-lifecycle">
            Imported into {stage.imported.participant_name}&rsquo;s note as version{' '}
            {stage.imported.version}.{' '}
            {stage.imported.resolution === 'IMPORTED'
              ? 'They had no note before this one.'
              : 'The previous content is still in the note&rsquo;s history.'}
          </p>
          <div className="actions">
            <button type="button" onClick={() => setStage({ kind: 'idle' })}>
              Import another
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

/** What the Host reads before confirming. */
function Preview({
  preview,
  busy,
  onConfirm,
  onDiscard,
}: {
  readonly preview: RemoteSubmissionPreview;
  readonly busy: boolean;
  readonly onConfirm: () => void;
  readonly onDiscard: () => void;
}) {
  return (
    <div className="import-preview">
      <p className="meta">
        From <code>{preview.origin}</code>
      </p>

      {preview.blocker !== null && (
        <p className="notice notice-validation" role="alert">
          {BLOCKER_MESSAGE[preview.blocker]}
          {preview.blocker === 'invalid_content' && preview.content_problem !== null
            ? ` ${CONTENT_PROBLEM[preview.content_problem] ?? ''}`
            : ''}
          {preview.blocker === 'duplicate' && preview.duplicate_note_version !== null
            ? ` It is already version ${preview.duplicate_note_version} of their note.`
            : ''}
        </p>
      )}

      {preview.blocker === null && !preview.participant_name_matches && (
        <p className="notice notice-conflict" role="alert">
          The submission calls this participant{' '}
          <strong>{preview.submission_participant_name}</strong>, but the roster
          calls them <strong>{preview.participant_name}</strong>. The roster is
          what counts. Check that this is the right person before importing.
        </p>
      )}

      {preview.blocker === null && preview.is_stale && (
        <p className="notice notice-lifecycle">
          This form was made from version {preview.source_version} and the note is
          now at version {preview.current_version}. Importing replaces the current
          content; nothing is lost, because every version is kept.
        </p>
      )}

      <dl className="facts">
        <div>
          <dt>Participant</dt>
          <dd>{preview.participant_name ?? '— not on this roster —'}</dd>
        </div>
        <div>
          <dt>Meeting</dt>
          <dd>
            {preview.meeting_title}
            {preview.meeting_matches ? '' : ' — the submission names a different meeting'}
          </dd>
        </div>
        <div>
          <dt>Note version</dt>
          <dd>
            {preview.current_version === null
              ? 'No note yet'
              : `Currently version ${preview.current_version}`}
            {' · '}form made from version {preview.source_version}
          </dd>
        </div>
        <div>
          <dt>Submission ID</dt>
          <dd>
            <code>{preview.submission_id}</code>
          </dd>
        </div>
        <div>
          <dt>Exported</dt>
          {/* The participant's own clock, so it is shown as reported. */}
          <dd>{preview.submitted_at} (as reported by their device)</dd>
        </div>
      </dl>

      <h4>The submitted note</h4>
      {/* Hostile input, rendered through the shared editor: DOM nodes, never
          markup, so there is no HTML sink here. */}
      <div className="import-note">
        <Markdown content={preview.submitted_note} />
      </div>

      {preview.current_note !== null && (
        <details className="import-current">
          <summary>What the note says now</summary>
          <div className="import-note">
            <Markdown content={preview.current_note} />
          </div>
        </details>
      )}

      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={busy || !preview.eligible}
          onClick={onConfirm}
        >
          {busy ? 'Importing…' : 'Import this note'}
        </button>
        <button type="button" disabled={busy} onClick={onDiscard}>
          Discard
        </button>
      </div>
    </div>
  );
}
