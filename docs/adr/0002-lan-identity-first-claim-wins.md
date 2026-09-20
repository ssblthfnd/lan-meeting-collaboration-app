# 0002. LAN identity binding: first-claim-wins

- Status: Accepted
- Date: 2026-09-20

## Context

A LAN participant joins by opening a URL or scanning a QR code and then picking
their name from a list the Host prepared. Nothing in that flow proves who they
are. Anyone who can reach the join URL - which is displayed on a projector -
could pick any identity, including one belonging to someone who has already
written notes.

Options considered:

1. **Per-participant claim code** issued by the Host. Strongest, but the Host
   must distribute 99 codes, which defeats the "scan and join" value.
2. **First-claim-wins binding**, with Host visibility and control.
3. **Host approves every join**. Safe but a bottleneck at the start of a
   meeting, when all participants join at once.

## Decision

MVP uses **first-claim-wins**, with Host oversight:

- A participant identity may be bound to exactly one active session at a time.
- A claim on an already-bound identity is rejected by the backend.
- The Host sees claim status per participant and can approve, reject or revoke.
- Binding is enforced in the backend. A `participant_id` supplied by the browser
  is never treated as authority.
- After binding, the session token is the credential. Only its hash is stored.
- Reconnection uses the existing session credential and cannot change identity.
- Claims and claim status changes are audited.

Option 3 is retained as a Host-controlled addition (approve/reject), not as a
mandatory gate on every join.

## Consequences

- The common failure - two people picking the same name, or someone taking a
  colleague's identity casually - is prevented.
- A determined attacker already on the LAN who obtains the join URL can still
  claim an *unclaimed* identity first. This is accepted for MVP; the join
  URL/QR is treated as an operational secret.
- LAN transport remains plain HTTP, so session tokens cross the network in the
  clear. This is a separate, stated boundary (see the architecture rules,
  section 14.1) and is not solved by this ADR.
- `participant_sessions` needs revocation semantics (`revoked_at`) and the
  claim status must be derivable for the Host UI.
