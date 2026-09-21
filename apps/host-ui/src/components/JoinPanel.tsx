import { useState } from 'react';
import type {
  HostError,
  JoinTokenIssued,
  MeetingDetail,
  MeetingId,
} from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';
import { useQuery } from '../hooks/useQuery';
import { ErrorNotice } from './ErrorNotice';
import { QrCode } from './QrCode';

/**
 * LAN access for one meeting: the server, the address, and the join link.
 *
 * # The server is started here, by a person
 *
 * Opening a meeting does not start it (ADR-0016). Binding a LAN-reachable
 * socket raises the Windows Firewall prompt and makes this machine answer on the
 * network, which should happen because the Host chose it.
 *
 * # The Host picks the address
 *
 * A laptop can be on Wi-Fi, Ethernet and a VPN at once and the server answers on
 * all of them, but the join URL can only name one. Only the person in the room
 * knows which network the participants are on, so the list is offered rather
 * than guessed. Loopback is offered too, clearly marked, because it is genuinely
 * useful for trying the participant page on this machine - and useless for
 * anyone else (architecture rules section 4).
 *
 * # Reachability cannot be proven from in here
 *
 * A bound socket does not mean a phone across the room can reach it: the
 * firewall, the network profile and client isolation on the access point all sit
 * in between and none is visible to this process. So the panel says what it
 * knows and what to check, rather than claiming success it cannot verify.
 */
export function JoinPanel({ meeting }: { readonly meeting: MeetingDetail }) {
  const meetingId: MeetingId = meeting.id;
  const server = useQuery(() => hostApi.lanServerStatus(), []);
  const interfaces = useQuery(() => hostApi.listLanInterfaces(), []);

  const [address, setAddress] = useState<string | null>(null);
  const [port, setPort] = useState('8765');
  const [issued, setIssued] = useState<JoinTokenIssued | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<HostError | null>(null);
  const [copied, setCopied] = useState(false);

  const running = server.data?.running ?? false;
  const boundPort = server.data?.port ?? null;
  const available = interfaces.data ?? [];
  // Preselect the first address, which the backend has already ordered with
  // private LAN addresses first and loopback last.
  const chosen = address ?? available[0]?.address ?? null;
  const chosenIsLoopback =
    available.find((candidate) => candidate.address === chosen)?.is_loopback ?? false;

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      return true;
    } catch (rejection) {
      setError(rejection as HostError);
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function start() {
    const parsed = Number.parseInt(port, 10);
    await run(async () => {
      await hostApi.startLanServer(Number.isNaN(parsed) ? null : parsed);
      server.reload();
    });
  }

  async function stop() {
    await run(async () => {
      await hostApi.stopLanServer();
      server.reload();
      // The link only works while the server runs, so showing it afterwards
      // would be showing something that does not work.
      setIssued(null);
    });
  }

  async function issue() {
    if (chosen === null) {
      return;
    }
    const parsed = boundPort ?? Number.parseInt(port, 10);
    await run(async () => {
      setIssued(await hostApi.issueJoinToken(meetingId, chosen, parsed));
      setCopied(false);
    });
  }

  async function copy() {
    if (issued === null) {
      return;
    }
    try {
      await navigator.clipboard.writeText(issued.join_url);
      setCopied(true);
    } catch {
      // Clipboard access can be refused; the URL is on screen to type.
      setCopied(false);
    }
  }

  if (meeting.status !== 'OPEN') {
    return (
      <section className="panel">
        <header className="panel-head">
          <h3>LAN access</h3>
        </header>
        <p className="notice notice-lifecycle">
          {meeting.status === 'DRAFT'
            ? 'Open this meeting before inviting participants. A join link can only be issued for an open meeting.'
            : 'This meeting is locked. Participants can no longer join.'}
        </p>
      </section>
    );
  }

  return (
    <section className="panel">
      <header className="panel-head">
        <h3>LAN access</h3>
        <span className="meta">
          {running ? `Server running on port ${boundPort ?? '?'}` : 'Server stopped'}
        </span>
      </header>

      {error && <ErrorNotice error={error} />}
      {server.error && <ErrorNotice error={server.error} />}

      {server.data?.ui_bundled === false && (
        <p className="notice notice-unexpected">
          The participant page was not built into this application. Run{' '}
          <code>npm run build:lan</code> and start the host again, or participants
          will see a blank page.
        </p>
      )}

      <div className="actions">
        {running ? (
          <button type="button" disabled={busy} onClick={() => void stop()}>
            {busy ? 'Working…' : 'Stop server'}
          </button>
        ) : (
          <>
            <button
              type="button"
              className="primary"
              disabled={busy}
              onClick={() => void start()}
            >
              {busy ? 'Working…' : 'Start server'}
            </button>
            <label className="inline">
              <span>Port</span>
              <input
                value={port}
                onChange={(e) => setPort(e.target.value)}
                inputMode="numeric"
                size={6}
              />
            </label>
          </>
        )}
      </div>

      {running && (
        <>
          <label>
            <span>Address to give participants</span>
            <select
              value={chosen ?? ''}
              onChange={(e) => setAddress(e.target.value)}
              disabled={busy || available.length === 0}
            >
              {available.map((candidate) => (
                <option key={`${candidate.name}-${candidate.address}`} value={candidate.address}>
                  {candidate.address} — {candidate.name}
                  {candidate.is_loopback ? ' (this computer only)' : ''}
                </option>
              ))}
            </select>
            <small className="meta">
              Pick the network the participants are on. This computer answers on
              all of them, but the link can only name one.
            </small>
          </label>

          {chosenIsLoopback && (
            <p className="notice notice-lifecycle">
              This address only works on this computer. Participants on other
              devices will not be able to open it.
            </p>
          )}

          <div className="actions">
            <button
              type="button"
              className="primary"
              disabled={busy || chosen === null}
              onClick={() => void issue()}
            >
              {issued === null ? 'Create join link' : 'Create a new link'}
            </button>
            {issued !== null && (
              <span className="meta">
                Creating a new link stops the current one from working.
              </span>
            )}
          </div>

          {issued !== null && (
            <div className="join">
              {issued.replaced_previous && (
                <p className="notice notice-lifecycle">
                  The previous link has stopped working. Anyone still holding it
                  needs this new one.
                </p>
              )}

              <QrCode matrix={issued.qr} label="QR code for the meeting join link" />

              <p className="join-url">
                <code>{issued.join_url}</code>
              </p>

              <div className="actions">
                <button type="button" onClick={() => void copy()}>
                  {copied ? 'Copied' : 'Copy link'}
                </button>
              </div>

              <p className="meta">
                Treat this link as a secret: anyone who has it can take a name
                from this meeting. If participants cannot connect, check that
                Windows Firewall allows this app on private networks and that
                everyone is on the same network.
              </p>
            </div>
          )}
        </>
      )}
    </section>
  );
}
