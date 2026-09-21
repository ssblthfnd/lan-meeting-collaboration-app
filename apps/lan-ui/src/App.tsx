import { useCallback, useEffect, useState } from 'react';
import type {
  LanError,
  LanJoinView,
  LanSessionView,
  ParticipantId,
} from '@lan-meeting/contracts';

import * as lanApi from './api/lanApi';
import { forgetToken, joinTokenFromUrl, rememberToken, storedToken } from './api/session';
import { IdentityPicker } from './components/IdentityPicker';
import { JoinedView } from './components/JoinedView';
import { Notice } from './components/Notice';

/**
 * LAN participant shell.
 *
 * Three states, decided by what credential the browser holds:
 *
 * ```text
 * session token  ->  Joined
 * join token     ->  IdentityPicker  -> (claim) -> Joined
 * neither        ->  "open the link the host gave you"
 * ```
 *
 * Nothing here holds authority. The backend resolves the session token to an
 * identity on every request; a `participant_id` sent from this bundle is a
 * target, never a claim about who is asking (architecture rules section 14.1).
 *
 * Note writing is not part of this step - it needs the shared editor and the
 * Markdown subset (ADR-0007), which arrive with their own roadmap step. A
 * participant who has joined sees their identity and the meeting's details.
 */
type Screen =
  | { readonly kind: 'loading' }
  | { readonly kind: 'no-link' }
  | { readonly kind: 'picking'; readonly join: LanJoinView }
  | { readonly kind: 'joined'; readonly session: LanSessionView }
  | { readonly kind: 'failed'; readonly error: LanError };

export function App() {
  const [screen, setScreen] = useState<Screen>({ kind: 'loading' });
  const [claiming, setClaiming] = useState<ParticipantId | null>(null);
  const [claimError, setClaimError] = useState<LanError | null>(null);
  const joinToken = joinTokenFromUrl();

  /** Load the identity list for the join token in the URL. */
  const loadJoin = useCallback(async (token: string) => {
    try {
      setScreen({ kind: 'picking', join: await lanApi.fetchJoin(token) });
    } catch (rejection) {
      setScreen({ kind: 'failed', error: rejection as LanError });
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function decide() {
      // A stored token is tried first: reconnecting must land on the same
      // identity rather than offering the picker again (ADR-0002 rule 10).
      if (storedToken() !== null) {
        try {
          const session = await lanApi.fetchSession();
          if (!cancelled) {
            setScreen({ kind: 'joined', session });
          }
          return;
        } catch {
          // Revoked, or for a meeting that has moved on. Drop it rather than
          // retrying a credential the server has already rejected.
          forgetToken();
        }
      }

      if (cancelled) {
        return;
      }
      if (joinToken === null) {
        setScreen({ kind: 'no-link' });
        return;
      }
      await loadJoin(joinToken);
    }

    void decide();
    return () => {
      cancelled = true;
    };
  }, [joinToken, loadJoin]);

  async function claim(participantId: ParticipantId) {
    if (joinToken === null) {
      return;
    }
    setClaiming(participantId);
    setClaimError(null);
    try {
      const claimed = await lanApi.claimIdentity(joinToken, participantId);
      rememberToken(claimed.session_token);
      setScreen({
        kind: 'joined',
        session: {
          participant: claimed.participant,
          meeting: claimed.meeting,
          // A fresh claim is never acknowledged yet, and it does not need to be:
          // approval is acknowledgement, not a gate (ADR-0016).
          acknowledged: false,
        },
      });
    } catch (rejection) {
      const error = rejection as LanError;
      setClaimError(error);
      // Someone took the name in between. Re-read the list so it shows who is
      // still free rather than leaving a stale screen.
      if (error.kind === 'identity_already_claimed') {
        await loadJoin(joinToken);
      }
    } finally {
      setClaiming(null);
    }
  }

  return (
    <main className="shell">
      {screen.kind === 'loading' && <p className="meta">Loading…</p>}

      {screen.kind === 'no-link' && (
        <>
          <h1>Join a meeting</h1>
          <p>
            Open the link or scan the QR code the meeting host gave you. This page
            needs that link to know which meeting you are joining.
          </p>
        </>
      )}

      {screen.kind === 'failed' && (
        <>
          <h1>Join a meeting</h1>
          <Notice error={screen.error} />
        </>
      )}

      {screen.kind === 'picking' && (
        <IdentityPicker
          join={screen.join}
          claiming={claiming}
          error={claimError}
          onClaim={(id) => void claim(id)}
        />
      )}

      {screen.kind === 'joined' && <JoinedView session={screen.session} />}
    </main>
  );
}
