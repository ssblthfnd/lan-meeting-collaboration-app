import type { LanSessionView } from '@lan-meeting/contracts';

import { MeetingHeader } from './MeetingHeader';

/**
 * Confirmation that you are in the meeting, and as whom.
 *
 * # What is deliberately not here
 *
 * Note writing. It needs the shared editor and the GFM subset (ADR-0007), which
 * arrive with their own roadmap step, and a half-built editor would be worse
 * than an honest "not yet". The screen says so rather than leaving someone
 * waiting for something to appear.
 *
 * # On acknowledgement
 *
 * `acknowledged` is shown as information, never as a state to wait out. A
 * participant whose claim the host has not acknowledged is fully joined and will
 * be able to do everything any other participant can: approval is
 * acknowledgement, not a permission gate (ADR-0016). Nothing on this screen is
 * disabled because of it, and nothing ever should be.
 */
export function JoinedView({ session }: { readonly session: LanSessionView }) {
  const { participant, meeting } = session;
  const details = [participant.department, participant.position, participant.meeting_role]
    .filter((value): value is string => value !== null)
    .join(' · ');

  return (
    <>
      <MeetingHeader meeting={meeting} />

      <section className="panel">
        <h2>You have joined</h2>
        <p className="joined-name">{participant.name}</p>
        {details !== '' && <p className="meta">{details}</p>}

        <p className="meta">
          This device is now signed in as you for this meeting. Keep this tab
          open — closing it means picking your name again.
        </p>

        {!session.acknowledged && (
          <p className="meta">
            The host has not marked your name as checked yet. You do not need to
            wait for that.
          </p>
        )}
      </section>

      <section className="panel">
        <h2>Notes</h2>
        <p className="empty">
          Writing notes is not available yet. It arrives in a later version of
          the app, together with the shared editor.
        </p>
      </section>
    </>
  );
}
