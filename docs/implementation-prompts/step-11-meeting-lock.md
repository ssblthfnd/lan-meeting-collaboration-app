# Step 11 — Meeting Lock

> **Frozen execution specification.** This document is self-contained. A future
> session should be able to read this single file and execute Step 11 without
> repeating the design inspection.

---

## 1. Execution Status

- **Design: inspected and FROZEN.**
- **Implementation: NOT started.**
- **Step 10 (Remote submission import): COMPLETE, committed and pushed.**
- **Baseline commit: `4d646b621b220f0c30545d627665096539e6db8a`**
  (`feat: add remote submission import`).
- `origin/main` synchronized with `main` at freeze time.
- Working tree was **clean** at freeze time.
- **This document is an execution specification, not a suggestion.**

---

## 2. Objective

Expose the already-implemented `Domain::lock_meeting` through the **Host Tauri
command surface** and the **Host UI**.

This is an **exposure / integration step, NOT a domain redesign.**

The lock rule, its transaction, its audit record, its event, its database
columns and its participant-facing behaviour all already exist and already
work. What is missing is a way for the Host to reach it from the application.

---

## 3. Existing Implementation — DO NOT REDESIGN

Every statement in this section was verified by reading the repository at the
baseline commit. Treat it as established fact, not as something to re-derive.

### 3.1 The domain method

`Domain::lock_meeting` already exists in `crates/app-core/src/service.rs`
(around line 898):

```rust
pub fn lock_meeting(
    &self,
    actor: &Actor,
    meeting_id: MeetingId,
) -> DomainResult<MeetingTransitioned>
```

It delegates to the existing private `transition()` helper (around line 921),
passing `MeetingStatus::Locked`, `Operation::LockMeeting` and
`AuditAction::MeetingLocked`, and then publishes `DomainEvent::MeetingLocked`.

### 3.2 The existing transaction sequence

Inside one `BEGIN IMMEDIATE`:

```text
find meeting                      -> MeetingNotFound if absent
  -> authorize                    -> the Authorized proof
  -> ensure_mutable               -> the lock, re-read here
  -> ensure_transition(OPEN -> LOCKED)
  -> set status + locked_at       -> set_meeting_status(&proof, ...)
  -> insert audit                 -> metadata { "from": ..., "to": ... }
COMMIT
  -> publish MeetingLocked
```

Return value: `MeetingTransitioned { meeting_id, from, to, at }`.

### 3.3 Authorization order

The order is **`authorize` → `ensure_mutable`**.

This ordering is **inherited from the shipped implementation and MUST NOT be
changed.** It is the same order `Domain::write_note` has used since Step 8 and
the same order ADR-0022 records for the import gate. `app-server` maps
`Forbidden` and `MeetingLocked` onto different participant-facing answers, so
swapping them would change what an unauthorized LAN participant learns about a
locked meeting.

### 3.4 What already exists

- `Actor::Host` is the **only** actor allowed to perform `LockMeeting`.
- `Operation::LockMeeting` already exists in `crates/app-core/src/authz.rs`,
  with `action() == "lock a meeting"` and `target() == "the meeting"`, and is
  listed in the suite's `HOST_ONLY` array.
- `AuditAction::MeetingLocked` already exists and renders as `"meeting.locked"`,
  already pinned by `action_strings_are_namespaced_and_stable`.
- `DomainEvent::MeetingLocked { meeting_id, at }` already exists
  (`crates/app-core/src/event.rs`), `variant_index() == 3`,
  `kind() == "meeting.locked"`, `audience() == Audience::Meeting(meeting_id)`.
  It is the **only** lifecycle event that is not `HostOnly`, deliberately.
- `MeetingTransitionedDto { meeting_id, from, to, at }` already exists in
  `src-tauri/src/dto.rs` and is already returned by `open_meeting`.
- `MeetingStatus` (`DRAFT | OPEN | LOCKED`), `MeetingTransitioned` and
  `DomainEventKind` (including `'meeting.locked'`) already exist in
  `packages/contracts`. The `AuditAction` union already includes
  `'meeting.locked'`.
- `V1__initial_schema.sql` already contains `status`, `locked_at` and
  `CHECK ((status = 'LOCKED') = (locked_at IS NOT NULL))`.
  `app-db`'s `set_meeting_status` already sets `locked_at` exactly when the
  status becomes `LOCKED`.
- `DomainError::MeetingLocked` and `DomainError::InvalidTransition` already map
  through `HostErrorKind` to `ErrorCategory::Lifecycle`, with the JSON kind
  `"meeting_locked"`.
- `apps/host-ui/src/components/AuditLog.tsx` already carries the human label
  `'meeting.locked': 'Meeting locked'`.
- `apps/host-ui/src/components/StatusBadge.tsx` already describes `LOCKED` as
  "Locked. Nothing can be changed."
- `MeetingDetailView.tsx` already renders a `Locked` metadata row whenever
  `detail.locked_at !== null`, and already shows a lifecycle notice reading
  "This meeting is locked. Nothing can be changed."

### 3.5 Lifecycle facts

- **There is no unlock transition.** `ensure_transition(MeetingStatus::Draft)`
  returns `InvalidTransition` unconditionally.
- **`DRAFT -> LOCKED` remains invalid**, answered as
  `InvalidTransition { expected: "OPEN", detected: Draft }`.
- **`OPEN -> LOCKED` is the legal lock transition.**
- **Locking an already-locked meeting answers `MeetingLocked`, not
  `InvalidTransition`**, because `ensure_mutable` runs before
  `ensure_transition`.
- **`LOCKED` meetings reject mutation through the existing `ensure_mutable`
  gates**, of which there are nine in `service.rs`: `update_meeting`,
  `add_participant`, `update_participant`, `remove_participant`,
  `issue_join_token`, `claim_identity`, `revoke_session`, `transition` (serving
  both `open_meeting` and `lock_meeting`) and the free function
  `authorize_note_write` (serving both `write_note` and
  `import_remote_submission`).

### 3.6 Participant-facing behaviour already implemented

- **LAN participants already receive `meeting.locked`** — the audience is
  `Audience::Meeting`, and `apps/lan-ui/src/App.tsx`'s realtime handler falls
  through to `reread()` for every kind other than `session.revoked` and
  `note.changed`.
- **LAN sockets remain connected.** `crates/app-server/src/ws.rs` documents
  this explicitly: locking broadcasts and disconnects nobody.
- **The participant UI already becomes read-only while remaining readable.**
  `JoinedView.tsx` announces the lock; `NotePanel.tsx` removes the editing
  controls and keeps the note visible; the LAN note **read** route is
  deliberately available while `LOCKED`.
- **Remote submission import already refuses locked meetings**, both in the
  read-only preview (`ImportBlockerDto::MeetingLocked`) and inside the import
  transaction, where the lock is decided **before** artefact identity — pinned
  by three precedence tests and by the structural guard
  `the_import_gate_decides_before_artefact_identity`.
- **Remote form generation already refuses a locked meeting** in
  `src-tauri/src/host.rs`. That check is documented as a courtesy, *not*
  enforcement, because generation persists nothing.

### 3.7 The only gap

`src-tauri/src/host.rs` currently carries a module-doc paragraph under
"What is deliberately not here" stating that `lock_meeting` is not exposed and
that adding the command should be a deliberate act. **Step 11 is that
deliberate act.** A grep for `lock_meeting` across `src-tauri/src/` returns
exactly that one doc-comment hit and nothing else.

**No domain changes are required.**

### 3.8 DO NOT MODIFY

- `Domain::lock_meeting`
- `transition()`
- `Meeting::ensure_mutable`
- `Meeting::ensure_transition`
- `Operation::LockMeeting`
- `AuditAction::MeetingLocked`
- `DomainEvent::MeetingLocked`
- event audience rules
- database schema
- existing LAN protocol
- participant locked-state implementation

---

## 4. Frozen Design Decisions

### D1 — Confirmation

Use a **two-step in-component React confirmation**.

Initial state: a button reading **"Lock meeting"**.

The confirmation state must clearly communicate that:

- locking ends editing for the Host **and** every participant;
- remote submissions can no longer be imported;
- the action **cannot be undone**.

Provide **Cancel**.

- Do **NOT** use `window.confirm`.
- Do **NOT** add a dialog plugin.
- Do **NOT** modify Tauri capabilities.

### D2 — Button styling

Reuse the **existing primary action styling** (the same treatment the
"Open meeting" button uses).

Do **NOT** introduce a new `danger` CSS class or any new styling system.

### D3 — Host event subscription

Do **NOT** add a `MeetingDetailView` subscription for `meeting.locked`.

The initiating Host action already calls `refresh()`. No second-Host-window
architecture is being introduced.

### D4 — ADR

Do **NOT** create ADR-0023. No new architectural decision is introduced by
Step 11.

### D5 — Existing `host_commands` test

`src-tauri/tests/host_commands.rs` contains
`a_locked_meeting_refuses_through_the_command_layer_too`, whose comment states
that locking is not exposed as a command on purpose and therefore reaches
`LOCKED` via `host.state.domain().lock_meeting(...)`.

Update it. Make it reach `LOCKED` **through the new command layer** rather than
directly through `Domain::lock_meeting`, where practical, and correct the stale
comment.

### D6 — No-unlock structural guard

Do **NOT** add the proposed repository-wide no-unlock source guard in
`boundaries.rs`. The existing domain test `there_is_no_unlock` already
establishes the invariant.

### D7 — Documentation sweep

During the documentation phase, update stale **current-state** references
caused by Step 10 and Step 11.

Do **NOT** rewrite historical Step 10 scope or non-goal statements that were
correct at the time they were written (this is the same treatment ADR-0008
receives).

### D8 — RemoteImport authorization

Add explicit test coverage proving `Actor::RemoteImport` cannot perform
`Operation::LockMeeting`.

Do **NOT** change authorization logic.

### D9 — Manual verification

Verify the real locked lifecycle as far as tooling permits.

Do **not** claim a Tauri WebView click-through if it was not actually
performed.

---

## 5. Required Runtime Changes

**Only these runtime changes are allowed.**

### 5.1 Rust — `src-tauri/src/host.rs`

- Remove the stale module-doc statement that `lock_meeting` is deliberately not
  exposed. (The neighbouring paragraph about note-version restore stays — that
  one is still true.)
- Add:

  ```rust
  pub fn lock_meeting(&self, meeting_id: &str) -> HostResult<MeetingTransitionedDto>
  ```

- Mirror the existing `open_meeting` method exactly.
- Parse the `MeetingId` via `crate::dto::parse_meeting_id`.
- Construct `Actor::Host` here, at the transport boundary.
- Call the existing `Domain::lock_meeting`.
- Convert the outcome into `MeetingTransitionedDto`.
- **No pre-flight status check.** A status read before the call is a race, not
  enforcement. The lock is re-read inside the domain transaction.
- **No direct database access.**

### 5.2 Rust — `src-tauri/src/commands.rs`

Add, beside `open_meeting`:

```rust
#[tauri::command(rename_all = "snake_case")]
pub fn lock_meeting(
    state: State<'_, HostState>,
    meeting_id: String,
) -> HostResult<MeetingTransitionedDto> {
    state.lock_meeting(&meeting_id)
}
```

One-line delegation, no logic.

### 5.3 Rust — `src-tauri/src/lib.rs`

Register `commands::lock_meeting` in `generate_handler![]`, beside
`commands::open_meeting`. The list goes from 28 entries to 29.

### 5.4 Rust — `src-tauri/tests/boundaries.rs`

Update the registered-command / gateway parity count in
`every_registered_command_is_reachable_from_the_host_ui_gateway` from **28** to
**29**.

This is the **only existing boundary guard Step 11 touches.** No guard asserts
that `lock_meeting` is absent from `src-tauri`; this was verified at freeze
time.

### 5.5 Tests — `src-tauri/tests/host_commands.rs`

Add or adjust command-layer coverage for:

1. Host locks an `OPEN` meeting successfully — returns `from: OPEN, to: LOCKED`;
   the detail view reports `LOCKED` and a non-null `locked_at` matching the
   returned `at`.
2. `DRAFT -> LOCKED` is rejected as `InvalidTransition`; status stays `DRAFT`;
   no audit row is written.
3. `LOCKED -> LOCKED` is rejected as `MeetingLocked` (JSON kind
   `"meeting_locked"`, message naming the meeting title); `locked_at` unchanged.
4. An unknown meeting is rejected correctly (`MeetingNotFound`).
5. A malformed meeting id is rejected **before** domain invocation.
6. A successful lock writes **exactly one** `meeting.locked` audit entry, whose
   metadata contains `OPEN` and `LOCKED`, visible through
   `list_audit_entries`.
7. The existing locked-mutation command-layer test reaches `LOCKED` through the
   **new command** (see D5).
8. `Actor::RemoteImport` cannot perform `Operation::LockMeeting` (see D8).

Do not duplicate domain tests unnecessarily. The domain suite already covers
locking, refusal, no-unlock, `locked_at` persistence and
rejected-transition-writes-no-audit; those tests must keep passing untouched.

### 5.6 Host gateway — `apps/host-ui/src/api/hostApi.ts`

- Add `lockMeeting(meetingId: MeetingId): Promise<MeetingTransitioned>`, placed
  beside `openMeeting`.
- Use the existing `call<T>()` / `invoke` gateway.
- Return `MeetingTransitioned` (the contract type already exists — do not add
  one).
- No direct `invoke` anywhere else; the guard
  `only_one_module_in_the_host_ui_calls_invoke` still applies.

### 5.7 Host UI — `apps/host-ui/src/components/MeetingDetailView.tsx`

- Add a lock handler mirroring the existing `open()`.
- Reuse the existing `busy` / `error` state and the existing `refresh()`.
- Render the lock action **only when `detail.status === 'OPEN'`**, in the
  `configuration` section, in the same position the "Open meeting" action
  occupies for `DRAFT`.
- Use the frozen two-step confirmation (D1).
- Reuse the existing primary styling (D2).
- A successful lock calls `refresh()`.
- Do **not** optimistically mutate state; nothing is rendered from the command
  response.
- No event subscription (D3).

### 5.8 LAN UI

Do **NOT** modify `apps/lan-ui` unless an actual existing bug is discovered. It
already handles the locked state correctly.

---

## 6. Existing Lifecycle Guarantees

All of the following must remain true after implementation:

- Host configuration mutations are refused after `LOCKED`.
- Participant mutations (add / update / remove) are refused after `LOCKED`.
- Host note writes are refused after `LOCKED`.
- Participant note writes over the LAN are refused after `LOCKED`.
- Remote submission import is refused after `LOCKED` — **as locked**, before
  artefact identity is consulted.
- Join-token issuance is refused after `LOCKED`.
- Identity claiming is refused after `LOCKED`.
- Session revocation is refused after `LOCKED`.
- **Reading notes remains allowed** over the LAN.
- **Existing participant sockets remain connected.**
- **Presence remains connection state**, unaffected by the lock.
- **The lock cannot be undone.**
- `locked_at` is atomically persisted with the `LOCKED` status, satisfying the
  schema's `CHECK ((status = 'LOCKED') = (locked_at IS NOT NULL))`.
- `MeetingLocked` is emitted **only after** a successful commit.

**No new lifecycle behaviour may be invented.**

Note for the implementer: after Step 11, `LOCKED` becomes reachable outside the
test suites for the first time, so every refusal path above becomes live in
production. The enforcement is already tested; the residual risk is UI clarity,
not rule failure. That is what section 11 targets.

---

## 7. Explicit Non-Goals

Step 11 must **NOT** include:

- changes to `app-core` lifecycle logic
- unlock / reopen functionality
- a database migration (migrations stay `V1` and `V2`)
- a Rust dependency
- an npm dependency
- a Tauri capability change
- a CSP change
- a new contract type
- a new DTO
- a new error variant
- a new domain event
- a LAN lock route
- WebSocket protocol changes
- participant authority to lock
- remote-form changes
- remote-import behaviour changes
- note-format changes
- export functionality
- Markdown export
- TXT export
- AI Context export
- PDF
- any AI API
- cloud or network functionality
- unrelated UI refactor
- any Step 12 implementation

---

## 8. Documentation Changes

**After** runtime implementation and verification, update only the stale
current-state documentation.

### 8.1 `docs/implementation-prompts/phase-1.md`

Update:

- the Current Execution Point block — it still says Step 10 "is implemented and
  awaiting review in the working tree" and records `HEAD = e867bdc…`; both were
  already stale before Step 11 began;
- the Step 10 row's commit column — it still reads `uncommitted`; the actual
  commit is `4d646b6`;
- the Step 11 row's status and commit;
- the Step 11 section body, which currently reads "Details beyond the above are
  **TBD**";
- any other current-roadmap / current-state references.

**Preserve** historical Step 10 scope and non-goal statements, including the
Step 10 non-goal line reading "no Step 11 lock implementation" — that was
correct for Step 10 and stays.

### 8.2 `README.md`

Update:

- the status blockquote, which still says reading submissions back **and** the
  meeting lock are not implemented — import shipped in Step 10, and the lock
  ships in Step 11;
- the roadmap `*(current)*` marker, which currently sits on item 10 and moves
  appropriately;
- the meeting lock becomes implemented after Step 11.

Do **not** rewrite unrelated README content.

### 8.3 ADRs

Do **NOT** create ADR-0023 (D4). Do not edit any existing ADR.

---

## 9. Implementation Sequence

Execute in this order. Each phase should compile and be independently
reviewable.

**Phase 11.1 — Backend exposure**
`host.rs` → `commands.rs` → `lib.rs` → boundary parity count.
(The count must move in this phase, because the guard fails the moment the
handler list grows.)

**Phase 11.2 — Backend tests**
`host_commands.rs` coverage; `RemoteImport` authorization coverage; the stale
test comment and its path to `LOCKED`.

**Phase 11.3 — Host gateway**
`hostApi.lockMeeting`. This is where the command/gateway parity guard goes
green again.

**Phase 11.4 — Host UI**
lock handler; two-step confirmation; `OPEN`-only visibility; `refresh()` after
success.

**Phase 11.5 — Verification**
full Rust validation; full TypeScript / UI validation; existing boundary and
offline guards; manual lifecycle verification.

**Phase 11.6 — Documentation**
`phase-1.md`; `README.md`.

**Phase 11.7 — Review**
full diff audit; scope audit; confirm no Step 12 leakage.

**Do NOT commit or push until explicit approval is given.**

---

## 10. Validation Requirements

Run the repository's established validation suite, including:

- `cargo fmt --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- npm typecheck / the repository equivalent
- `npm run check`
- Vitest
- production build checks
- the boundary guards (`src-tauri/tests/boundaries.rs`)
- the offline guard (`scripts/check-remote-form-offline.mjs`, via
  `npm run build:remote`) — it must not be weakened
- `git diff --check`

**Report exact results**, including failures. Do not summarise a failing run as
passing.

---

## 11. Manual Verification

Where possible:

1. Create a meeting.
2. Open the meeting.
3. Lock it through the Host UI.
4. Verify the Host UI changes to `LOCKED`.
5. Verify `locked_at` appears.
6. Verify the lock action disappears.
7. Verify a connected LAN participant receives `meeting.locked` **without a
   reload**.
8. Verify the participant's note remains **readable**.
9. Verify participant note **editing is blocked**.
10. Verify remote submission import is blocked.
11. Verify connected sockets **remain connected**.

### If Tauri WebView automation is unavailable

This is the expected case: browser automation drives Chrome, not the Tauri
WebView, so the Host UI button cannot be clicked programmatically.

- Do **not** pretend the Host UI click was tested.
- Verify the actual Host command / state path through the available Rust /
  Tauri test mechanism (the pattern used in Steps 9 and 10: a driver calling
  the real `HostState` method).
- Verify the participant side in a **real browser** against a running LAN
  server.
- **Clearly state the limitation** in the report.

---

## 12. Stop Conditions

**STOP and report instead of improvising** if:

- `Domain::lock_meeting` differs materially from this specification;
- `transition()` needs modification;
- a migration appears necessary;
- a dependency appears necessary;
- a capability change appears necessary;
- a contract / DTO / error / event change appears necessary;
- an existing locked-state invariant fails;
- a broader UI refactor appears necessary;
- a new architectural decision is required;
- implementation would require changing Step 12 scope.

**Do not silently redesign the architecture.**

---

## 13. Final Review Report

Before requesting commit approval, report:

- changed files
- files intentionally untouched
- implementation summary
- tests added / changed
- complete validation results
- manual verification results
- tooling limitations
- documentation changes
- boundary audit
- migration / dependency / capability audit
- `git diff --check`
- current `git status`
- current `HEAD`
- `main` vs `origin/main`

**Do NOT commit or push.**

End with exactly:

```text
STEP 11 IMPLEMENTATION COMPLETE — READY FOR REVIEW
```

---

## 14. Execution Rule

A future session reading this file must treat its contents as **FROZEN**.

Do **not** repeat the Step 11 design inspection unless a stop condition in
section 12 is hit.

Start by:

1. reading this file;
2. checking the repository baseline against the recorded checkpoint
   (`4d646b621b220f0c30545d627665096539e6db8a`);
3. executing the implementation sequence in section 9.

If the repository has moved beyond commit `4d646b6`, first inspect the current
state and reconcile **only what is necessary** to preserve the frozen design.
**Do not discard later legitimate work.**

This document itself was the only artifact created in the freeze turn.
