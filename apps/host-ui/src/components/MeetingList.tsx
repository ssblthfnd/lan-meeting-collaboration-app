import type { MeetingId, MeetingSummary } from '@lan-meeting/contracts';

import { StatusBadge } from './StatusBadge';

/**
 * The Host's meetings, in the order the backend returned them.
 *
 * The order is not re-sorted here. The query has an explicit total ordering
 * (architecture rules section 26.4) and re-sorting in the UI would make the
 * displayed order depend on which layer ran last.
 */
export function MeetingList({
  meetings,
  onSelect,
}: {
  readonly meetings: readonly MeetingSummary[];
  readonly onSelect: (id: MeetingId) => void;
}) {
  if (meetings.length === 0) {
    return (
      <p className="empty">
        No meetings yet. Create one to start preparing a roster.
      </p>
    );
  }

  return (
    <ul className="card-list">
      {meetings.map((meeting) => (
        <li key={meeting.id}>
          <button type="button" className="card" onClick={() => onSelect(meeting.id)}>
            <span className="card-head">
              <span className="card-title">{meeting.title}</span>
              <StatusBadge status={meeting.status} />
            </span>
            <span className="card-meta">
              {/* The timezone is always shown with the schedule: a date and time
                  without it is ambiguous (PRD section 25.3). */}
              {meeting.date} · {meeting.start_time}–{meeting.end_time} ({meeting.timezone})
            </span>
            <span className="card-meta">
              {meeting.topic ?? 'No topic'} ·{' '}
              {meeting.participant_count === 1
                ? '1 participant'
                : `${meeting.participant_count} participants`}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
