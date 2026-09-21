import type { MeetingStatus } from '@lan-meeting/contracts';

/** What each lifecycle status means for the Host, in one place. */
const DESCRIPTION: Record<MeetingStatus, string> = {
  DRAFT: 'Being prepared. Configuration and roster can still be changed.',
  OPEN: 'Open. Configuration and roster are settled.',
  LOCKED: 'Locked. Nothing can be changed.',
};

export function StatusBadge({ status }: { readonly status: MeetingStatus }) {
  return (
    <span className={`badge badge-${status.toLowerCase()}`} title={DESCRIPTION[status]}>
      {status}
    </span>
  );
}

/** The same text as prose, for a detail view that has room for it. */
export function statusDescription(status: MeetingStatus): string {
  return DESCRIPTION[status];
}
