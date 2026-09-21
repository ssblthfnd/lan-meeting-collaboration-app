import type { LanError } from '@lan-meeting/contracts';

/**
 * A refusal from the LAN server, shown as the server phrased it.
 *
 * The backend writes these messages for a participant, so they are shown rather
 * than rewritten here - which also means there is one wording to improve rather
 * than two to keep in step.
 *
 * Some refusals suggest a next step the message itself cannot know, such as
 * reloading after the host reopens a meeting. Those are added below the message,
 * never in place of it.
 */
export function Notice({ error }: { readonly error: LanError }) {
  return (
    <div className={`notice notice-${error.category}`} role="alert">
      <p>{error.message}</p>
      {error.kind === 'identity_already_claimed' && (
        <p className="meta">
          If that is your name, ask the host to release it and then try again.
        </p>
      )}
      {error.kind === 'unauthenticated' && (
        <p className="meta">Open the join link again to pick your name.</p>
      )}
    </div>
  );
}
