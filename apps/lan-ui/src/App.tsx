/**
 * LAN participant shell.
 *
 * Skeleton only. The join flow, identity claim (first-claim-wins), note editing
 * and link entry are Phase 1 work and are not implemented yet.
 *
 * This bundle never holds authority: the participant identity is established by
 * the backend from a session token, and a participant id sent from here is
 * never trusted.
 */
export function App() {
  return (
    <main className="shell">
      <h1>Join Meeting</h1>
      <p className="subtitle">LAN participant - project skeleton</p>
      <p>
        No features are implemented yet. Identity binding, notes and realtime
        updates arrive in Phase 1.
      </p>
    </main>
  );
}
