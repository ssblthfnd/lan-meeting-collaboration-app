import { useState } from 'react';
import type { HostError, MeetingConfigurationInput, MeetingId } from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { useQuery } from '../hooks/useQuery';
import { AuditLog } from './AuditLog';
import { ErrorNotice } from './ErrorNotice';
import { ExportPanel } from './ExportPanel';
import { ImportSubmissionPanel } from './ImportSubmissionPanel';
import { JoinPanel } from './JoinPanel';
import { MeetingForm } from './MeetingForm';
import { NotesPanel } from './NotesPanel';
import { ParticipantRoster } from './ParticipantRoster';
import { StatusBadge, statusDescription } from './StatusBadge';

/**
 * One meeting: its configuration, its roster and its audit trail.
 *
 * Which of the three sections is visible is the only thing this component
 * decides for itself. Everything else it shows comes from the backend, and every
 * action it offers is re-judged there.
 */
type Section =
  | 'configuration'
  | 'participants'
  | 'access'
  | 'notes'
  | 'remote'
  | 'export'
  | 'audit';

export function MeetingDetailView({
  meetingId,
  onBack,
  onChanged,
}: {
  readonly meetingId: MeetingId;
  readonly onBack: () => void;
  /** Lets the meeting list re-read itself after a change here. */
  readonly onChanged: () => void;
}) {
  const meeting = useQuery(() => hostApi.getMeeting(meetingId), [meetingId]);
  const [section, setSection] = useState<Section>('configuration');
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<HostError | null>(null);
  const [confirmingLock, setConfirmingLock] = useState(false);

  if (meeting.error) {
    return (
      <>
        <button type="button" onClick={onBack}>
          ← All meetings
        </button>
        <ErrorNotice error={meeting.error} />
      </>
    );
  }

  if (!meeting.data) {
    return <p className="meta">Loading…</p>;
  }

  const detail = meeting.data;
  const editable = detail.status === 'DRAFT';

  /** Refresh both this view and the list behind it from the backend. */
  function refresh() {
    meeting.reload();
    onChanged();
  }

  async function save(configuration: MeetingConfigurationInput) {
    setBusy(true);
    setError(null);
    try {
      await hostApi.updateMeeting(meetingId, configuration);
      setEditing(false);
      refresh();
    } catch (rejection) {
      setError(rejection as HostError);
    } finally {
      setBusy(false);
    }
  }

  async function open() {
    setBusy(true);
    setError(null);
    try {
      await hostApi.openMeeting(meetingId);
      refresh();
    } catch (rejection) {
      setError(rejection as HostError);
    } finally {
      setBusy(false);
    }
  }

  async function lock() {
    setBusy(true);
    setError(null);
    try {
      await hostApi.lockMeeting(meetingId);
      setConfirmingLock(false);
      refresh();
    } catch (rejection) {
      setError(rejection as HostError);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <button type="button" className="back" onClick={onBack}>
        ← All meetings
      </button>

      <header className="detail-head">
        <div>
          <h2>{detail.title}</h2>
          <p className="meta">
            {detail.date} · {detail.start_time}–{detail.end_time} ({detail.timezone})
          </p>
        </div>
        <StatusBadge status={detail.status} />
      </header>

      <p className="meta">{statusDescription(detail.status)}</p>

      {error && <ErrorNotice error={error} />}

      <nav className="tabs">
        {(
          ['configuration', 'participants', 'access', 'notes', 'remote', 'export', 'audit'] as const
        ).map((name) => (
          <button
            key={name}
            type="button"
            className={section === name ? 'tab active' : 'tab'}
            onClick={() => setSection(name)}
          >
            {name === 'configuration' && 'Configuration'}
            {name === 'participants' && `Participants (${detail.participant_count})`}
            {name === 'access' && 'LAN access'}
            {name === 'notes' && 'Notes'}
            {name === 'remote' && 'Remote'}
            {name === 'export' && 'Export'}
            {name === 'audit' && 'Audit'}
          </button>
        ))}
      </nav>

      {section === 'configuration' &&
        (editing ? (
          <MeetingForm
            initial={{
              title: detail.title,
              topic: detail.topic ?? '',
              date: detail.date,
              start_time: detail.start_time,
              end_time: detail.end_time,
              timezone: detail.timezone,
              location: detail.location ?? '',
              description: detail.description ?? '',
            }}
            submitLabel="Save configuration"
            busy={busy}
            error={error}
            onSubmit={(configuration) => void save(configuration)}
            onCancel={() => {
              setEditing(false);
              setError(null);
            }}
          />
        ) : (
          <section className="panel">
            <header className="panel-head">
              <h3>Configuration</h3>
              {editable ? (
                <button type="button" onClick={() => setEditing(true)}>
                  Edit
                </button>
              ) : (
                <span className="meta">Settled</span>
              )}
            </header>

            {!editable && (
              <p className="notice notice-lifecycle">
                {detail.status === 'LOCKED'
                  ? 'This meeting is locked. Nothing can be changed.'
                  : 'This meeting is open. Its configuration was settled when it left DRAFT.'}
              </p>
            )}

            <dl className="metadata">
              <div>
                <dt>Topic</dt>
                <dd>{detail.topic ?? '—'}</dd>
              </div>
              <div>
                <dt>Date</dt>
                <dd>{detail.date}</dd>
              </div>
              <div>
                <dt>Start</dt>
                <dd>{detail.start_time}</dd>
              </div>
              <div>
                <dt>End</dt>
                <dd>{detail.end_time}</dd>
              </div>
              <div>
                {/* Always stated explicitly, never implied by the reader's clock
                    (PRD section 25.3). */}
                <dt>Timezone</dt>
                <dd>{detail.timezone}</dd>
              </div>
              <div>
                <dt>Location</dt>
                <dd>{detail.location ?? '—'}</dd>
              </div>
              <div>
                <dt>Description</dt>
                <dd>{detail.description ?? '—'}</dd>
              </div>
              <div>
                <dt>Created</dt>
                <dd>{detail.created_at}</dd>
              </div>
              <div>
                <dt>Updated</dt>
                <dd>{detail.updated_at}</dd>
              </div>
              {detail.locked_at !== null && (
                <div>
                  <dt>Locked</dt>
                  <dd>{detail.locked_at}</dd>
                </div>
              )}
            </dl>

            {editable && (
              <div className="actions">
                <button type="button" className="primary" disabled={busy} onClick={() => void open()}>
                  {busy ? 'Working…' : 'Open meeting'}
                </button>
                <span className="meta">
                  Opening settles the configuration and the roster. There is no way
                  back to <code>DRAFT</code>.
                </span>
              </div>
            )}

            {detail.status === 'OPEN' &&
              (confirmingLock ? (
                <div className="actions">
                  <button type="button" className="primary" disabled={busy} onClick={() => void lock()}>
                    {busy ? 'Working…' : 'Yes, lock this meeting'}
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => setConfirmingLock(false)}
                  >
                    Cancel
                  </button>
                  <span className="meta">
                    Locking ends editing for you and every participant, and remote
                    submissions can no longer be imported. This cannot be undone.
                  </span>
                </div>
              ) : (
                <div className="actions">
                  <button
                    type="button"
                    className="primary"
                    disabled={busy}
                    onClick={() => setConfirmingLock(true)}
                  >
                    Lock meeting
                  </button>
                  <span className="meta">
                    Locking ends editing for everyone and cannot be undone.
                  </span>
                </div>
              ))}
          </section>
        ))}

      {section === 'participants' && (
        <ParticipantRoster meeting={detail} onRosterChanged={refresh} />
      )}

      {section === 'access' && <JoinPanel meeting={detail} />}

      {section === 'notes' && <NotesPanel meeting={detail} />}

      {/* Remote participation is meeting-scoped: a form is generated for one
          participant, but a submission is read back into this meeting. Import
          therefore lives here rather than on a roster row (ADR-0022). */}
      {section === 'remote' && (
        <ImportSubmissionPanel meeting={detail} onImported={refresh} />
      )}

      {section === 'export' && <ExportPanel meeting={detail} />}

      {section === 'audit' && <AuditLog meetingId={meetingId} />}
    </>
  );
}
