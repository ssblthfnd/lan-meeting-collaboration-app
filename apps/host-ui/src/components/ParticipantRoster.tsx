import { useState } from 'react';
import { MAX_PARTICIPANTS } from '@lan-meeting/contracts';
import type {
  HostError,
  MeetingDetail,
  MeetingId,
  ParticipantDetailsInput,
  ParticipantId,
  ParticipantSummary,
} from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { useQuery } from '../hooks/useQuery';
import { ErrorNotice } from './ErrorNotice';

/**
 * The participant roster (PRD section 7).
 *
 * Editable only while the meeting is `DRAFT` (ADR-0013). When it is not, the
 * controls are absent and the reason is stated - but that is presentation, not
 * enforcement: every one of these commands is refused by the backend on a
 * meeting that has moved on, whatever this component renders
 * (architecture rules section 15).
 *
 * The count is shown against the maximum of 99. The limit is the backend's, held
 * to inside a transaction; showing it here only saves the Host from finding out
 * by being refused.
 */

const EMPTY: ParticipantDetailsInput = {
  name: '',
  department: null,
  position: null,
  meeting_role: null,
};

/** A blank field is absent, not an empty string. */
function optional(value: string): string | null {
  const trimmed = value.trim();
  return trimmed === '' ? null : trimmed;
}

function ParticipantFields({
  values,
  onChange,
}: {
  readonly values: ParticipantDetailsInput;
  readonly onChange: (values: ParticipantDetailsInput) => void;
}) {
  return (
    <div className="row">
      <label>
        <span>Name</span>
        <input
          required
          value={values.name}
          onChange={(e) => onChange({ ...values, name: e.target.value })}
          placeholder="Budi Santoso"
        />
      </label>
      <label>
        <span>Department</span>
        <input
          value={values.department ?? ''}
          onChange={(e) => onChange({ ...values, department: optional(e.target.value) })}
          placeholder="Finance"
        />
      </label>
      <label>
        <span>Position</span>
        <input
          value={values.position ?? ''}
          onChange={(e) => onChange({ ...values, position: optional(e.target.value) })}
          placeholder="Analyst"
        />
      </label>
      <label>
        <span>Meeting role</span>
        <input
          value={values.meeting_role ?? ''}
          onChange={(e) => onChange({ ...values, meeting_role: optional(e.target.value) })}
          placeholder="Note taker"
        />
      </label>
    </div>
  );
}

export function ParticipantRoster({
  meeting,
  onRosterChanged,
}: {
  readonly meeting: MeetingDetail;
  /** Lets the detail view re-read its own participant count from the backend. */
  readonly onRosterChanged: () => void;
}) {
  const meetingId: MeetingId = meeting.id;
  const editable = meeting.status === 'DRAFT';

  const roster = useQuery(() => hostApi.listParticipants(meetingId), [meetingId]);
  const [draft, setDraft] = useState<ParticipantDetailsInput>(EMPTY);
  const [editing, setEditing] = useState<ParticipantId | null>(null);
  const [editValues, setEditValues] = useState<ParticipantDetailsInput>(EMPTY);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<HostError | null>(null);

  const participants = roster.data ?? [];
  const full = participants.length >= MAX_PARTICIPANTS;

  /**
   * Run a mutation, then re-read from the backend.
   *
   * The refreshed roster always comes from a query. Nothing is patched into
   * local state from the request that was just sent: the database is
   * authoritative, and a locally assembled "this is probably how it looks now"
   * is the frontend holding state it has no right to.
   */
  async function mutate(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      roster.reload();
      onRosterChanged();
      return true;
    } catch (rejection) {
      setError(rejection as HostError);
      return false;
    } finally {
      setBusy(false);
    }
  }

  function startEditing(participant: ParticipantSummary) {
    setEditing(participant.id);
    setError(null);
    setEditValues({
      name: participant.name,
      department: participant.department,
      position: participant.position,
      meeting_role: participant.meeting_role,
    });
  }

  return (
    <section className="panel">
      <header className="panel-head">
        <h3>Participants</h3>
        <span className="meta">
          {participants.length} of {MAX_PARTICIPANTS}
        </span>
      </header>

      {!editable && (
        <p className="notice notice-lifecycle">
          The roster was settled when this meeting left <code>DRAFT</code>. It is
          now <code>{meeting.status}</code> and participants can no longer be
          added, changed or removed.
        </p>
      )}

      {roster.error && <ErrorNotice error={roster.error} />}
      {error && <ErrorNotice error={error} />}

      {roster.loading && participants.length === 0 && <p className="meta">Loading…</p>}

      {participants.length === 0 && !roster.loading ? (
        <p className="empty">No participants yet.</p>
      ) : (
        <ul className="rows">
          {participants.map((participant) => (
            <li key={participant.id}>
              {editing === participant.id ? (
                <form
                  className="form"
                  onSubmit={async (event) => {
                    event.preventDefault();
                    const ok = await mutate(() =>
                      hostApi.updateParticipant(meetingId, participant.id, editValues),
                    );
                    if (ok) {
                      setEditing(null);
                    }
                  }}
                >
                  <ParticipantFields values={editValues} onChange={setEditValues} />
                  <div className="actions">
                    <button type="submit" className="primary" disabled={busy}>
                      Save
                    </button>
                    <button type="button" onClick={() => setEditing(null)} disabled={busy}>
                      Cancel
                    </button>
                  </div>
                </form>
              ) : (
                <div className="row-item">
                  <div>
                    <strong>{participant.name}</strong>
                    <span className="card-meta">
                      {[
                        participant.department,
                        participant.position,
                        participant.meeting_role,
                      ]
                        .filter((value): value is string => value !== null)
                        .join(' · ') || 'No further details'}
                    </span>
                  </div>
                  {editable && (
                    <div className="actions">
                      <button type="button" onClick={() => startEditing(participant)} disabled={busy}>
                        Edit
                      </button>
                      <button
                        type="button"
                        className="danger"
                        disabled={busy}
                        onClick={() =>
                          void mutate(() =>
                            hostApi.removeParticipant(meetingId, participant.id),
                          )
                        }
                      >
                        Remove
                      </button>
                    </div>
                  )}
                </div>
              )}
            </li>
          ))}
        </ul>
      )}

      {editable && (
        <form
          className="form"
          onSubmit={async (event) => {
            event.preventDefault();
            const ok = await mutate(() => hostApi.addParticipant(meetingId, draft));
            if (ok) {
              setDraft(EMPTY);
            }
          }}
        >
          <h4>Add a participant</h4>
          <ParticipantFields values={draft} onChange={setDraft} />
          <div className="actions">
            <button type="submit" className="primary" disabled={busy || full}>
              {busy ? 'Saving…' : 'Add participant'}
            </button>
            {full && (
              <span className="meta">
                This meeting already has the maximum of {MAX_PARTICIPANTS} participants.
              </span>
            )}
          </div>
        </form>
      )}
    </section>
  );
}
