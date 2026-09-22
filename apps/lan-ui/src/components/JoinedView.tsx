import type { LanError, LanNoteView, LanSessionView } from '@lan-meeting/contracts';

import { MeetingHeader } from './MeetingHeader';
import { NotePanel } from './NotePanel';

/**
 * Confirmation that you are in the meeting, and as whom.
 *
 * # On acknowledgement
 *
 * `acknowledged` is shown as information, never as a state to wait out. A
 * participant whose claim the host has not acknowledged is fully joined and will
 * be able to do everything any other participant can: approval is
 * acknowledgement, not a permission gate (ADR-0016). Nothing on this screen is
 * disabled because of it, and nothing ever should be.
 *
 * # On the lock
 *
 * A locked meeting is announced here because the participant should know why
 * things stop working, and for no other reason. This banner is **not** the
 * enforcement: the backend re-reads the meeting's status inside every mutating
 * transaction, so a participant who never received the event, closed this tab,
 * or edited the bundle is refused just the same (architecture rules section
 * 15). The socket keeps the screen honest; it does not keep the meeting safe.
 */
export function JoinedView({
  session,
  note,
  noteLoading,
  noteChangedElsewhere,
  onSaveNote,
  onReloadNote,
  onDismissNoteChange,
  onNoteEditingChange,
}: {
  readonly session: LanSessionView;
  readonly note: LanNoteView | null;
  readonly noteLoading: boolean;
  readonly noteChangedElsewhere: boolean;
  readonly onSaveNote: (content: string) => Promise<LanError | null>;
  readonly onReloadNote: () => void;
  readonly onDismissNoteChange: () => void;
  readonly onNoteEditingChange: (editing: boolean) => void;
}) {
  const { participant, meeting } = session;
  const details = [participant.department, participant.position, participant.meeting_role]
    .filter((value): value is string => value !== null)
    .join(' · ');

  return (
    <>
      <MeetingHeader meeting={meeting} />

      {meeting.status === 'LOCKED' && (
        <div className="notice notice-lifecycle" role="status">
          <p>The host has locked this meeting. Nothing can be changed now.</p>
          <p className="meta">
            You can stay on this page; there is just nothing left to do here.
          </p>
        </div>
      )}

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

      <NotePanel
        meeting={meeting}
        note={note}
        loading={noteLoading}
        changedElsewhere={noteChangedElsewhere}
        onSave={onSaveNote}
        onReload={onReloadNote}
        onDismissChange={onDismissNoteChange}
        onEditingChange={onNoteEditingChange}
      />
    </>
  );
}
