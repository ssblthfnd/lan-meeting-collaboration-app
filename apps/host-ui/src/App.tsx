import { MAX_PARTICIPANTS } from '@lan-meeting/contracts';

/**
 * Host dashboard shell.
 *
 * Skeleton only. Meeting creation, participant management, the join QR, the
 * audit view, import and export are Phase 1 work and are not implemented yet.
 */
export function App() {
  return (
    <main className="shell">
      <h1>LAN Meeting Collaboration App</h1>
      <p className="subtitle">Host dashboard - project skeleton</p>
      <p>
        No features are implemented yet. The backend is authoritative for
        authorization, meeting lock and audit; SQLite is the source of truth.
      </p>
      <p className="meta">Participants per meeting: 1-{MAX_PARTICIPANTS}</p>
    </main>
  );
}
