import { useState } from 'react';
import type { HostError, MeetingConfigurationInput, MeetingId } from '@lan-meeting/contracts';

import * as hostApi from './api/hostApi';
import { ErrorNotice } from './components/ErrorNotice';
import { MeetingDetailView } from './components/MeetingDetailView';
import { MeetingForm, emptyMeetingValues } from './components/MeetingForm';
import { MeetingList } from './components/MeetingList';
import { useQuery } from './hooks/useQuery';

/**
 * Host dashboard.
 *
 * Two views - the meeting list and one meeting - selected by state rather than by
 * a router, because two views do not need one.
 *
 * Nothing important lives here. Every meeting, participant and audit entry on
 * screen was read from SQLite through a command, and every action is decided in
 * `app-core` inside a transaction. The UI enables and disables controls to make
 * the state legible, and that is presentation: a disabled button is not
 * enforcement (architecture rules section 15), so the backend refuses the same
 * action again regardless of what this code renders.
 */
export function App() {
  const meetings = useQuery(() => hostApi.listMeetings(), []);
  const [selected, setSelected] = useState<MeetingId | null>(null);
  const [creating, setCreating] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<HostError | null>(null);

  async function create(configuration: MeetingConfigurationInput) {
    setBusy(true);
    setError(null);
    try {
      const created = await hostApi.createMeeting(configuration);
      setCreating(false);
      // Re-read the list rather than appending the new meeting locally: the
      // database is authoritative about what exists and in what order.
      meetings.reload();
      setSelected(created.meeting_id);
    } catch (rejection) {
      setError(rejection as HostError);
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="shell">
      <header className="app-head">
        <h1>LAN Meeting Collaboration App</h1>
        <p className="subtitle">Host dashboard</p>
      </header>

      {selected !== null ? (
        <MeetingDetailView
          meetingId={selected}
          onBack={() => setSelected(null)}
          onChanged={() => meetings.reload()}
        />
      ) : (
        <>
          <section className="panel">
            <header className="panel-head">
              <h2>Meetings</h2>
              {!creating && (
                <button type="button" className="primary" onClick={() => setCreating(true)}>
                  New meeting
                </button>
              )}
            </header>

            {error && !creating && <ErrorNotice error={error} />}

            {creating ? (
              <MeetingForm
                initial={emptyMeetingValues()}
                submitLabel="Create meeting"
                busy={busy}
                error={error}
                onSubmit={(configuration) => void create(configuration)}
                onCancel={() => {
                  setCreating(false);
                  setError(null);
                }}
              />
            ) : meetings.error ? (
              <ErrorNotice error={meetings.error} />
            ) : meetings.loading && meetings.data === null ? (
              <p className="meta">Loading…</p>
            ) : (
              <MeetingList meetings={meetings.data ?? []} onSelect={setSelected} />
            )}
          </section>
        </>
      )}
    </main>
  );
}
