import { useState } from 'react';
import type {
  HostError,
  MeetingDetail,
  ParticipantSummary,
  RemoteFormGenerated,
} from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { ErrorNotice } from './ErrorNotice';

/**
 * Generating a remote participation form for one participant (PRD section 10).
 *
 * For somebody who is not on the network: the Host generates a file, sends it
 * however they like, and the participant fills it in offline and sends back a
 * submission. Nothing in this application transmits either file — there is no
 * upload, no endpoint and no account (PRD section 5.3).
 *
 * # What this panel is not
 *
 * It does not import anything. Reading a submission back is a later step, with
 * its own preview and its own confirmation, and no command for it exists yet.
 * What is here is one button and the path it produces.
 *
 * # The path is the backend's
 *
 * The file is written by Rust into the application's own data folder, and the
 * path comes back to be displayed. This window cannot choose a location: there
 * is no filesystem or dialog permission in the app's capability set, and adding
 * one would be its own architectural decision (ADR-0021 decision 10).
 */
export function RemoteFormPanel({
  meeting,
  participant,
  onClose,
}: {
  readonly meeting: MeetingDetail;
  readonly participant: ParticipantSummary;
  readonly onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<HostError | null>(null);
  const [result, setResult] = useState<RemoteFormGenerated | null>(null);

  async function generate() {
    setBusy(true);
    setError(null);
    try {
      setResult(await hostApi.generateRemoteForm(meeting.id, participant.id));
    } catch (rejection) {
      setResult(null);
      setError(rejection as HostError);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="remote-form">
      <h4>Remote form for {participant.name}</h4>

      <p className="meta">
        Generates one HTML file that opens offline. Send it to {participant.name}{' '}
        by email, chat or a USB stick; they fill it in with no internet and send
        a submission file back.
      </p>

      {error !== null && <ErrorNotice error={error} />}

      {result === null ? (
        <div className="actions">
          <button type="button" className="primary" disabled={busy} onClick={() => void generate()}>
            {busy ? 'Generating…' : 'Generate remote form'}
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            Cancel
          </button>
        </div>
      ) : (
        <>
          <dl className="facts">
            <div>
              <dt>File</dt>
              {/* `<code>` renders as text, and the path came from the backend. */}
              <dd>
                <code>{result.file_name}</code>
              </dd>
            </div>
            <div>
              <dt>Saved in</dt>
              <dd>
                <code className="path">{result.path}</code>
              </dd>
            </div>
            <div>
              <dt>Submission ID</dt>
              <dd>
                <code>{result.submission_id}</code>
              </dd>
            </div>
            <div>
              <dt>Note version</dt>
              <dd>
                {result.source_version === 0
                  ? 'None yet — the form opens empty'
                  : `Version ${result.source_version}, already filled in`}
              </dd>
            </div>
          </dl>

          <p className="meta">
            The submission ID identifies this file. Generating another form makes
            a second, separate one; both stay usable, and re-importing the same
            file is detected rather than written twice.
          </p>

          <div className="actions">
            <button type="button" disabled={busy} onClick={() => void generate()}>
              Generate another
            </button>
            <button type="button" className="primary" onClick={onClose}>
              Done
            </button>
          </div>
        </>
      )}
    </div>
  );
}
