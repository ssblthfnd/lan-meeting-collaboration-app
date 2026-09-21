import type { LanError, LanJoinView, ParticipantId } from '@lan-meeting/contracts';

import { MeetingHeader } from './MeetingHeader';
import { Notice } from './Notice';

/**
 * Pick your name from the list the host prepared (PRD section 5.2).
 *
 * A taken name is shown and disabled rather than hidden: someone looking for
 * their own name needs to find it and see that it is already in use, which is
 * the cue to tell the host rather than to assume they were left off the list.
 *
 * Disabling is presentation only. The backend refuses a claim on a held
 * identity whatever this renders, and the database decides a race between two
 * browsers pressing at the same moment (ADR-0002, architecture rules 15).
 */
export function IdentityPicker({
  join,
  claiming,
  error,
  onClaim,
}: {
  readonly join: LanJoinView;
  /** The identity currently being claimed, if any. */
  readonly claiming: ParticipantId | null;
  readonly error: LanError | null;
  readonly onClaim: (id: ParticipantId) => void;
}) {
  const busy = claiming !== null;
  const available = join.identities.filter((identity) => !identity.claimed).length;

  return (
    <>
      <MeetingHeader meeting={join.meeting} />

      <section className="panel">
        <h2>Who are you?</h2>
        <p className="meta">
          {available === 0
            ? 'Every name on this list is already in use.'
            : 'Choose your name to join. You can only pick one.'}
        </p>

        {error && <Notice error={error} />}

        {join.identities.length === 0 ? (
          <p className="empty">
            The host has not added anyone to this meeting yet.
          </p>
        ) : (
          <ul className="identities">
            {join.identities.map((identity) => (
              <li key={identity.id}>
                <button
                  type="button"
                  className={identity.claimed ? 'identity taken' : 'identity'}
                  disabled={identity.claimed || busy}
                  onClick={() => onClaim(identity.id)}
                >
                  <span className="identity-name">{identity.name}</span>
                  <span className="meta">
                    {claiming === identity.id
                      ? 'Joining…'
                      : identity.claimed
                        ? 'Already joined'
                        : 'Tap to join'}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}
