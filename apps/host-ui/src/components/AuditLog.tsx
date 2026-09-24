import type { AuditAction, AuditEntry, MeetingId } from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { useQuery } from '../hooks/useQuery';
import { ErrorNotice } from './ErrorNotice';

/**
 * The meeting's audit trail.
 *
 * Read-only, and not merely by omission: the log is append-only, the database
 * refuses `UPDATE` and `DELETE` on it (ADR-0011), and no command exists to
 * change an entry. There is deliberately no action of any kind in this view.
 *
 * The order is the backend's - chronological with the id as tiebreaker
 * (architecture rules section 26.4) - and is not re-sorted here.
 */

/** A readable sentence per action, so the trail can be skimmed. */
const DESCRIPTION: Record<AuditAction, string> = {
  'meeting.created': 'Meeting created',
  'meeting.updated': 'Configuration changed',
  'meeting.opened': 'Meeting opened',
  'meeting.locked': 'Meeting locked',
  'participant.added': 'Participant added',
  'participant.updated': 'Participant changed',
  'participant.removed': 'Participant removed',
  'meeting.join_token_issued': 'Join link created',
  'participant.claimed': 'Participant joined',
  // "Checked", not "approved": acknowledgement grants nothing, and phrasing it
  // as approval would imply the participant had been waiting (ADR-0016).
  'participant.claim_approved': 'Participant checked by host',
  'participant.session_revoked': 'Name released by host',
  'note.created': 'Note created',
  'note.updated': 'Note changed',
  'meeting.exported': 'Meeting exported',
};

/** Who acted. `HOST` carries no participant id, by constraint (ADR-0008). */
function actor(entry: AuditEntry): string {
  return entry.actor_id === null
    ? entry.actor_type
    : `${entry.actor_type} ${entry.actor_id}`;
}

/**
 * Render metadata deterministically.
 *
 * Keys are sorted, so the same record always renders identically - the same
 * property exports rely on (architecture rules section 19). Values are rendered
 * as text through React, which escapes them: audit metadata contains
 * participant-supplied names and is treated as hostile input even in the Host UI
 * (architecture rules section 13.1).
 */
function metadataRows(metadata: Readonly<Record<string, unknown>>): [string, string][] {
  return Object.keys(metadata)
    .sort()
    .map((key) => {
      const value = metadata[key];
      const rendered =
        value === null || value === undefined
          ? '—'
          : typeof value === 'object'
            ? JSON.stringify(value)
            : String(value);
      return [key, rendered];
    });
}

export function AuditLog({ meetingId }: { readonly meetingId: MeetingId }) {
  const audit = useQuery(() => hostApi.listAuditEntries(meetingId), [meetingId]);
  const entries = audit.data ?? [];

  return (
    <section className="panel">
      <header className="panel-head">
        <h3>Audit trail</h3>
        <span className="meta">Append-only · {entries.length} entries</span>
      </header>

      {audit.error && <ErrorNotice error={audit.error} />}
      {audit.loading && entries.length === 0 && <p className="meta">Loading…</p>}

      {entries.length === 0 && !audit.loading ? (
        <p className="empty">Nothing recorded yet.</p>
      ) : (
        <ol className="trail">
          {entries.map((entry) => (
            <li key={entry.id}>
              <div className="row-item">
                <div>
                  <strong>{DESCRIPTION[entry.action] ?? entry.action}</strong>
                  <span className="card-meta">
                    <code>{entry.action}</code> · {entry.target_type}
                    {entry.target_id !== null && (
                      <>
                        {' '}
                        <code>{entry.target_id}</code>
                      </>
                    )}
                  </span>
                  <span className="card-meta">
                    {actor(entry)} · {entry.created_at}
                  </span>
                </div>
              </div>
              {entry.metadata !== null && (
                <dl className="metadata">
                  {metadataRows(entry.metadata).map(([key, value]) => (
                    <div key={key}>
                      <dt>{key}</dt>
                      <dd>{value}</dd>
                    </div>
                  ))}
                </dl>
              )}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
