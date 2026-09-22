import { useState } from 'react';
import type { ActorType, MeetingId, ParticipantId } from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { useQuery } from '../hooks/useQuery';
import { ErrorNotice } from './ErrorNotice';
import { Markdown } from './Markdown';

/**
 * A note's version history.
 *
 * **View-only.** There is no restore here, and not because it was forgotten:
 * version history is view-only in step 8 and restore is deferred (ADR-0019).
 * The database refuses `UPDATE` on `note_versions` and no command writes a
 * historical body back, so there is nothing for this view to offer beyond
 * looking.
 *
 * The list carries metadata only. A body crosses the boundary when the Host
 * opens one version, which is why the preview is a second query rather than a
 * field on the list.
 */

/** How each author reads in the list. */
const AUTHOR: Record<ActorType, string> = {
  HOST: 'Host',
  PARTICIPANT: 'Participant',
  REMOTE_IMPORT: 'Remote import',
};

/** A rough size, so a list can show how much a version carried. */
function size(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${Math.round(bytes / 1024)} KiB`;
}

export function NoteHistory({
  meetingId,
  participantId,
}: {
  readonly meetingId: MeetingId;
  readonly participantId: ParticipantId;
}) {
  const versions = useQuery(
    () => hostApi.listNoteVersions(meetingId, participantId),
    [meetingId, participantId],
  );
  const [open, setOpen] = useState<number | null>(null);

  const preview = useQuery(
    () =>
      open === null
        ? Promise.resolve(null)
        : hostApi.getNoteVersion(meetingId, participantId, open),
    [meetingId, participantId, open],
  );

  const rows = versions.data ?? [];

  return (
    <section className="panel">
      <header className="panel-head">
        <h4>Version history</h4>
        <span className="meta">
          {rows.length === 0 ? 'No versions yet' : `${rows.length} versions · view only`}
        </span>
      </header>

      {versions.error && <ErrorNotice error={versions.error} />}
      {versions.loading && rows.length === 0 && <p className="meta">Loading…</p>}

      {rows.length === 0 && !versions.loading ? (
        <p className="empty">This note has no history yet.</p>
      ) : (
        <ol className="rows">
          {rows.map((version) => (
            <li key={version.version}>
              <div className="row-item">
                <div>
                  <strong>Version {version.version}</strong>
                  <span className="card-meta">
                    {AUTHOR[version.created_by_type]} · {version.created_at} ·{' '}
                    {size(version.byte_length)}
                  </span>
                </div>
                <div className="actions">
                  <button
                    type="button"
                    onClick={() => setOpen(open === version.version ? null : version.version)}
                  >
                    {open === version.version ? 'Hide' : 'View'}
                  </button>
                </div>
              </div>

              {open === version.version && (
                <div className="version-preview">
                  {preview.error && <ErrorNotice error={preview.error} />}
                  {preview.loading && <p className="meta">Loading…</p>}
                  {preview.data !== null && preview.data !== undefined && (
                    <>
                      <p className="meta">
                        A read-only copy of this version. Restoring an old version is
                        not available.
                      </p>
                      <Markdown content={preview.data.content} />
                    </>
                  )}
                </div>
              )}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
