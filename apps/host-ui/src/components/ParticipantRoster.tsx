import { useState } from 'react';
import { MAX_PARTICIPANTS } from '@lan-meeting/contracts';
import type {
  DomainEventKind,
  HostError,
  Iso8601Utc,
  MeetingDetail,
  MeetingId,
  ParticipantClaimStatus,
  ParticipantDetailsInput,
  ParticipantId,
  ParticipantPresence,
  ParticipantSummary,
} from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { useDomainEvents } from '../hooks/useDomainEvents';
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
 *
 * # Presence, and what it is not
 *
 * The roster follows domain events and re-reads itself when one arrives, so a
 * participant opening or closing their browser shows up here without the Host
 * reloading anything. Two things this deliberately is not:
 *
 * - **Not authority.** Presence is an observation. Nothing the Host or a
 *   participant is allowed to do depends on it, and no control below is enabled
 *   or disabled by it.
 * - **Not a substitute for the claim status.** A name can be claimed and not
 *   connected - someone joined and closed the tab - and that is a different
 *   fact from having released the name. Both are shown.
 */

/** The events that change what this roster shows. */
const WATCHED: readonly DomainEventKind[] = [
  'participant.added',
  'participant.updated',
  'participant.removed',
  'participant.claimed',
  'claim.approved',
  'session.revoked',
  'presence.changed',
];

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

/**
 * How each claim state reads to the Host.
 *
 * `PENDING` is phrased as *joined*, because that is what it is: the participant
 * is in the meeting and working. Approval is acknowledgement, not a gate, so
 * this must never read as "waiting for you" (ADR-0016).
 */
const CLAIM_LABEL: Record<ParticipantClaimStatus, string> = {
  UNCLAIMED: 'Not joined',
  PENDING: 'Joined',
  CLAIMED: 'Joined · checked',
  REVOKED: 'Released',
};

/** Whether a live session currently holds this identity. */
function isHeld(status: ParticipantClaimStatus): boolean {
  return status === 'PENDING' || status === 'CLAIMED';
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

/**
 * Whether a participant has a browser open on this meeting right now.
 *
 * Silent when nobody is connected and nothing was ever recorded, so a roster
 * the Host has only just typed in does not sprout a column of "offline" labels
 * about people who have not been invited yet.
 *
 * `last_seen_at` is shown as the raw UTC timestamp, like every other system
 * timestamp in this window. It means the last moment a socket was observed to
 * open or close - not a heartbeat - so a connection that dropped silently is
 * noticed only when the server's keepalive notices it (ADR-0018).
 */
function PresenceBadge({
  presence,
}: {
  readonly presence: ParticipantPresence | undefined;
}) {
  if (presence === undefined) {
    return null;
  }
  if (presence.connected) {
    return <span className="badge badge-presence-connected">Connected</span>;
  }
  const lastSeen: Iso8601Utc | null = presence.last_seen_at;
  if (lastSeen === null) {
    return null;
  }
  return (
    <span className="badge badge-presence-away" title={`Last seen ${lastSeen}`}>
      Not connected
    </span>
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
  const presence = useQuery(
    () => hostApi.listParticipantPresence(meetingId),
    [meetingId],
  );
  const [draft, setDraft] = useState<ParticipantDetailsInput>(EMPTY);
  const [editing, setEditing] = useState<ParticipantId | null>(null);
  const [editValues, setEditValues] = useState<ParticipantDetailsInput>(EMPTY);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<HostError | null>(null);

  const participants = roster.data ?? [];
  const full = participants.length >= MAX_PARTICIPANTS;

  const presenceById = new Map<ParticipantId, ParticipantPresence>(
    (presence.data ?? []).map((row) => [row.participant_id, row]),
  );

  // An event says something changed; these two commands say what it is. The
  // payload is never rendered (ADR-0018).
  useDomainEvents(meetingId, WATCHED, () => {
    roster.reload();
    presence.reload();
  });

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
                    <strong>{participant.name}</strong>{' '}
                    <span
                      className={`badge badge-claim-${participant.claim_status.toLowerCase()}`}
                    >
                      {CLAIM_LABEL[participant.claim_status]}
                    </span>{' '}
                    <PresenceBadge presence={presenceById.get(participant.id)} />
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
                  <div className="actions">
                    {editable && (
                      <>
                        <button
                          type="button"
                          onClick={() => startEditing(participant)}
                          disabled={busy}
                        >
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
                      </>
                    )}

                    {/* Session controls, available once someone has joined.
                        They are not roster edits, so they stay available after
                        the meeting opens - which is exactly when they matter.
                        "Check" only records that the Host saw the claim; it
                        grants nothing (ADR-0016). "Release" ends the session so
                        the name can be claimed again, which is what makes
                        first-claim-wins workable in a room (ADR-0002 rule 5). */}
                    {isHeld(participant.claim_status) && (
                      <>
                        {participant.claim_status === 'PENDING' && (
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() =>
                              void mutate(() =>
                                hostApi.approveParticipantClaim(meetingId, participant.id),
                              )
                            }
                          >
                            Check
                          </button>
                        )}
                        <button
                          type="button"
                          className="danger"
                          disabled={busy}
                          onClick={() =>
                            void mutate(() =>
                              hostApi.revokeParticipantSession(meetingId, participant.id),
                            )
                          }
                        >
                          Release name
                        </button>
                      </>
                    )}
                  </div>
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
