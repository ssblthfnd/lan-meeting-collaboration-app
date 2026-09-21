import type { LanMeetingView } from '@lan-meeting/contracts';

/**
 * The meeting a participant is joining or has joined.
 *
 * The timezone is always shown beside the schedule and the times are never
 * converted to the device's own zone. A participant's phone may be set to
 * anywhere; the meeting happens at its own hours, and showing 09:00 in a zone
 * the meeting is not in would be worse than showing nothing
 * (PRD section 25.3).
 */
export function MeetingHeader({ meeting }: { readonly meeting: LanMeetingView }) {
  return (
    <header className="meeting">
      <h1>{meeting.title}</h1>
      {meeting.topic !== null && <p className="topic">{meeting.topic}</p>}
      <p className="meta">
        {meeting.date} · {meeting.start_time}–{meeting.end_time} ({meeting.timezone})
      </p>
      {meeting.location !== null && <p className="meta">{meeting.location}</p>}
    </header>
  );
}
