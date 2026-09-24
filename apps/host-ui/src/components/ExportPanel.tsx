import { useState } from 'react';
import type { ExportFormat, ExportGenerated, HostError, MeetingDetail } from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { ErrorNotice } from './ErrorNotice';

/**
 * Export the meeting record as Markdown, TXT or AI Context (PRD section 20).
 *
 * One button per format, each calling the same `generate_export` command
 * with a different `format` string - mirroring {@link RemoteFormPanel},
 * which already establishes the "click a button, Rust writes a file, show
 * the resulting path" pattern for this application.
 *
 * # This panel decides nothing
 *
 * Whether export is currently allowed is the backend's decision, re-checked
 * inside the command twice (Step 12): a `DRAFT` meeting's refusal arrives
 * here as an ordinary {@link HostError} and is shown exactly like any other
 * refusal. This component never hides the buttons based on `meeting.status`
 * as a substitute for that check - the UI is not the security boundary.
 */
const FORMATS: ReadonlyArray<{ readonly value: ExportFormat; readonly label: string }> = [
  { value: 'markdown', label: 'Markdown' },
  { value: 'txt', label: 'TXT' },
  { value: 'ai_context', label: 'AI Context' },
];

export function ExportPanel({ meeting }: { readonly meeting: MeetingDetail }) {
  const [busy, setBusy] = useState<ExportFormat | null>(null);
  const [error, setError] = useState<HostError | null>(null);
  const [result, setResult] = useState<ExportGenerated | null>(null);

  async function generate(format: ExportFormat) {
    setBusy(format);
    setError(null);
    try {
      setResult(await hostApi.generateExport(meeting.id, format));
    } catch (rejection) {
      setResult(null);
      setError(rejection as HostError);
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="panel">
      <header className="panel-head">
        <h3>Export</h3>
      </header>

      <p className="meta">
        Writes a record of this meeting - its configuration, roster and every
        participant&apos;s latest note - to a file on this computer. Nothing is
        uploaded and nothing is sent anywhere.
      </p>

      {error !== null && <ErrorNotice error={error} />}

      <div className="actions">
        {FORMATS.map((format) => (
          <button
            key={format.value}
            type="button"
            className="primary"
            disabled={busy !== null}
            onClick={() => void generate(format.value)}
          >
            {busy === format.value ? 'Generating…' : `Export as ${format.label}`}
          </button>
        ))}
      </div>

      {result !== null && (
        <dl className="facts">
          <div>
            <dt>File</dt>
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
        </dl>
      )}
    </section>
  );
}
