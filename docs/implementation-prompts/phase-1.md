# Phase 1 — LAN Meeting Collaboration App

Implementation prompt archive for Phase 1, in chronological order.

## What this file is, and is not

This file records **implementation prompts and execution scope** so that the
development sequence is documented, future sessions have one Phase 1 reference,
scope boundaries and architectural decisions are preserved, completed and
upcoming steps can be told apart, and the plan can be reviewed on its own.

It is **not** a replacement for any of these, and it never overrides them:

| Document | Remains authoritative for |
| --- | --- |
| `CLAUDE.md` | repository-level engineering instructions |
| `PRD-LAN-Meeting-Collaboration-App.md` | product requirements |
| `Architecture-Engineering-Rules-LAN-Meeting-Collaboration-App.md` | architecture and engineering rules |
| `docs/adr/` | accepted decisions and their rationale |

Where this file and any of the above disagree, the above wins and this file is
wrong and should be corrected.

---

## Phase Objective

Phase 1 delivers the MVP as a **self-contained, local-first desktop
application**. Concretely, it must:

- establish the self-contained desktop host application (Tauri 2 shell, Host UI,
  application state, command surface) with no cloud, no external API and no
  public hosting;
- establish the domain and database foundation — `app-core` as the only place
  business rules live, and SQLite as the single source of truth;
- establish the LAN participant flow — a local HTTP server, an embedded
  participant bundle, a join token and QR, identity claim and session
  authentication;
- establish the realtime collaboration infrastructure — committed domain events
  delivered on audience-scoped channels, with presence;
- progressively implement meeting, participant, note, remote-form, lock and
  export capabilities according to the approved roadmap.

The roadmap in `README.md` is the ordering this archive follows.

---

## Implementation Rules

These apply to every step in this file.

**Scope**

- Implement one step at a time.
- Do not silently expand scope.
- Do not implement deferred features unless explicitly approved.
- Do not change architecture decisions without an explicit decision; a changed
  decision is a new ADR, not an edit to an old one.

**Architecture**

- Domain rules remain authoritative in `app-core`.
- SQLite remains the source of truth.
- Transport layers must not bypass `app-core` mutation boundaries.
- Host UI communicates through Tauri commands.
- LAN participant UI communicates through HTTP and WebSocket only.
- Remote form remains self-contained and offline.
- Preserve the existing repository structure and architectural boundaries.

**Security**

- Security-sensitive authorization is enforced server-side, in Rust. UI hiding
  is not a security mechanism.
- Do not trust `participant_id` or any identity information supplied by the
  browser. The actor is established at the transport boundary.
- Credentials and tokens must never be persisted in plaintext.
- Never place credentials in URLs or query strings.

**Dependencies**

- Do not introduce unnecessary dependencies. New dependencies are an ADR-0006
  matter and need an explicit decision.

**Git**

- Keep Git identity as the repository owner's own identity.
- Do not add AI/Claude/Anthropic co-author trailers or attribution to commits or
  pull request descriptions.
- Do not commit or push unless explicitly requested by the user.

**Completion**

- Every implementation step must pass the applicable validation suite before it
  is considered complete.
- Do not weaken, skip or delete tests to make an implementation pass.

---

## Step Status

| Step | Scope | Status | Commit |
| --- | --- | --- | --- |
| Step 0 | Repository & toolchain foundation | COMPLETE | `8d844b8`, `272044f` |
| Step 1 | Database foundation | COMPLETE | `5e87d6d` |
| Step 2 | Domain rule engine | COMPLETE | `47ad5dc` |
| Step 3 | Meeting & participant management | COMPLETE | `6a8ea46` |
| Step 4 | Host UI foundation | COMPLETE | `d41c6d5` |
| Step 5 | Remote form foundation | COMPLETE | `8d844b8` (see note) |
| Step 6 | LAN server & participant join | COMPLETE | `334cdf2` |
| Step 7 | Realtime updates & participant presence | COMPLETE | `f5e1b1f` |
| Step 8 | Host note editing & version history | COMPLETE | uncommitted |
| Step 8B | Participant note editing & LAN note API | **NEXT** | — |
| Step 9 | Remote form generation | PLANNED | — |
| Step 10 | Remote submission import pipeline | PLANNED | — |
| Step 11 | Meeting lock | PLANNED | — |
| Step 12 | Export: Markdown and TXT / AI Context | PLANNED | — |

### Numbering note

Two numbering schemes exist in this repository and they are offset. Recording
the mapping here so a future reader does not have to rediscover it:

- `README.md` lists a 1-based roadmap of twelve items, where item 1 is
  "Environment and skeleton".
- This archive uses the 0-based step numbering used in the implementation
  prompts, where Step 0 is the repository and toolchain foundation.

They map as `README n` = `Step n-1` for `README` items 1 to 5, and they
**coincide from Step 6 onward** (`README 6` = Step 6, and so on through
`README 12` = Step 12).

**Step 5 has no dedicated commit.** The remote form foundation — the workspace,
the single-file build, the offline self-containment guard and ADR-0009 — landed
inside the skeleton commit `8d844b8` rather than as a separate feature step. It
is listed separately here because it is a distinct body of work with its own
accepted decision, not because there is a commit to point at.

**Step 8B did not exist in the original roadmap.** It was added as decision D3
during the Step 8 design review, because PRD section 15 requires participant
note editing and the `README` roadmap had no step for it.

---

## Step 0 — Repository & Toolchain Foundation

**Status: COMPLETE** (`8d844b8`, `272044f`)

### Intent

Stand up the whole repository shape at once, so that every later step has a
place to put its code and no step has to invent structure while also
implementing a feature.

### What was established

- Cargo workspace: `crates/app-core`, `crates/app-db`, `crates/app-server`,
  `crates/app-remote`, `crates/app-export`, and `src-tauri`.
- npm workspaces: `apps/host-ui`, `apps/lan-ui`, `apps/remote-form`,
  `packages/contracts`, `packages/editor`.
- `tsconfig.base.json` with `strict`, `noUncheckedIndexedAccess`,
  `exactOptionalPropertyTypes`, `verbatimModuleSyntax` and workspace path
  aliases.
- `rust-toolchain.toml`, `.gitattributes` line-ending normalization, `.gitignore`
  covering generated databases and build output.
- The authoritative documents: `CLAUDE.md`, the PRD, the architecture and
  engineering rules, `README.md`.
- ADR-0001 through ADR-0009, and `docs/adr/README.md` as the index.
- `scripts/check-remote-form-offline.mjs`, the build guard for the remote form.

### Major constraints recorded here

- Three UI bundles are separate on purpose and must not be merged. `host-ui` is
  never served over the LAN; `remote-form` never talks to the Host.
- Crate responsibilities are fixed: business rules only in `app-core`, SQL only
  in `app-db`, no business rules in either transport.
- No cloud service, external API, telemetry, authentication provider, relay or
  CDN resource, at any point, in any step.

---

## Step 1 — Database Foundation

**Status: COMPLETE** (`5e87d6d`)

### Intent

Make the schema the primary enforcement point for every invariant that can be
expressed in it, so that a rule which lives only in Rust cannot be skipped by a
future code path.

### What was established

- `crates/app-db/migrations/V1__initial_schema.sql`: `meetings`, `participants`,
  `participant_sessions`, `notes`, `note_links`, `note_versions`, `audit_logs`,
  `remote_submissions`.
- Embedded migrations via `refinery`, compiled into the binary so a Host machine
  needs no migration files on disk and no network access.
- `Db`: one writer connection behind a mutex plus a pool of read-only
  connections, WAL journal mode, `foreign_keys = ON`, busy timeout, and
  `PRAGMA` verification on every connection.
- `Db::write` / `write_with` opening `BEGIN IMMEDIATE`, taking the write lock up
  front.
- `Sql<T>` newtype for canonical storage conversions, and `DbError`.
- ADR-0010 (jiff, because it carries its own IANA tzdb, which matters on
  Windows) and ADR-0011 (identifier and canonical storage formats).

### Invariants the schema enforces by itself

- Ids are canonical UUIDv7, checked by `GLOB`.
- System timestamps are UTC, fixed width, so text ordering is time ordering.
- `UNIQUE(meeting_id, participant_id)` on `notes` — one note per participant.
- A note's participant belongs to the note's meeting, via a composite foreign
  key.
- At most one live session per identity, via a partial unique index on
  `revoked_at IS NULL`.
- `note_versions` is append-only: `UNIQUE(note_id, version)` plus a trigger
  refusing `UPDATE`.
- `audit_logs` refuses `UPDATE` and `DELETE` by trigger.
- At most five links per note, by trigger.

---

## Step 2 — Domain Rule Engine

**Status: COMPLETE** (`47ad5dc`)

### Intent

Create a single mutation boundary that a transport cannot go around, and make
"cannot go around" a compile-time fact rather than a review item.

### What was established

- `Actor` — `Host`, `Participant`, `Claimant`, `RemoteImport`. Minted at the
  transport boundary, never parsed from a request body.
- `authorize()` returning `Authorized`, whose fields are crate-private. Every
  write method on `DomainTx` requires one, and the persistence adapter that
  implements those methods cannot mint one.
- `Operation` enum, added alongside implementations rather than in advance.
- `Id<E>` (UUIDv7), the time types (`MeetingDate`, `MeetingTime`,
  `MeetingTimeZone`, `UtcTimestamp`), `TokenHash`.
- `MeetingStatus`, the lifecycle, and the lock rule as
  `Meeting::ensure_mutable` / `ensure_draft` / `ensure_open`.
- `AuditAction`, `AuditTarget`, `AuditEntry` — a typed vocabulary, so a typo is
  a compile error rather than an unqueryable trail.
- `port::{Database, DomainTx}` — the persistence seam. `app-core` has no SQLite
  dependency.
- `service::Domain` with the canonical mutation shape, including `write_note`.
- ADR-0012, the domain mutation boundary.

### Major constraint recorded here

The meeting status is re-read **inside** the mutating transaction, every time. A
pre-flight check is a race, not an enforcement.

---

## Step 3 — Meeting & Participant Management

**Status: COMPLETE** (`6a8ea46`)

### Intent

Implement the operations a Host needs before anybody can join, and settle when a
meeting's shape stops being editable.

### What was established

- `create_meeting` (always `DRAFT`), `update_meeting`, `open_meeting`.
- `add_participant`, `update_participant`, `remove_participant`.
- The 99-participant limit, counted inside the mutating transaction, with
  migration `V2__participant_roster_limit.sql` as the independent second
  enforcement point.
- Configuration validation, including schedule validation against the meeting's
  own timezone.
- ADR-0013 — a meeting's configuration and roster are settled in `DRAFT`.

### Major constraint recorded here

Configuration and roster are `DRAFT`-only. Notes are deliberately the opposite
case and stay writable through `OPEN`; this is why `ensure_draft` is a separate
check from `ensure_mutable` rather than a stricter version of it.

---

## Step 4 — Host UI Foundation

**Status: COMPLETE** (`d41c6d5`)

### Intent

Give the Host a window onto everything built so far, and fix the shape of the
Host boundary before more commands exist to get it wrong.

### What was established

- `apps/host-ui`: React 19 + Vite, rendered inside the Tauri WebView.
- `api/hostApi.ts` as the **single** Tauri gateway. No other module may call
  `invoke`, and a structural guard enforces it.
- `hooks/useQuery.ts` — about thirty lines, deliberately not a data-fetching
  library: SQLite is the source of truth and a cache is the wrong instinct.
- Components: `MeetingList`, `MeetingForm`, `MeetingDetailView`,
  `ParticipantRoster`, `AuditLog`, `ErrorNotice`, `StatusBadge`.
- `HostQueries` — the Host read path, on the read-only pool, not through the
  mutation boundary (ADR-0014).
- The Host error contract: `HostError` / `HostErrorKind` / `ErrorCategory`, a
  discriminated union keyed on `kind` (ADR-0015).
- `src-tauri/tests/boundaries.rs` — structural guards: no SQL or database
  primitives in either transport, one module owns `invoke`, every registered
  command is reachable, capabilities stay minimal, the CSP stays restrictive,
  the audit log is read-only from the command layer.

### Major constraint recorded here

Reads and writes take different paths on purpose. The Host may see everything in
their own database and a participant may not, so read models are named for their
audience and are never shared across audiences.

---

## Step 5 — Remote Form Foundation

**Status: COMPLETE** (landed in `8d844b8`; see the numbering note)

### Intent

Make the offline contract of the remote form a property the build checks, before
any feature depends on it.

### What was established

- `apps/remote-form` built with plain TypeScript and DOM APIs, **no UI
  framework** (ADR-0009).
- `vite-plugin-singlefile`, and `build.modulePreload: false` so Vite's preload
  polyfill emits no `fetch()`.
- `scripts/check-remote-form-offline.mjs`, run as part of `npm run build:remote`,
  which reads the **built bytes** and fails the build on: an external script or
  stylesheet, a remote `@import`, any absolute `http(s)://` URL other than
  `www.w3.org/`, `fetch(`, `XMLHttpRequest`, `new WebSocket`, `sendBeacon`,
  `new EventSource`, more than one HTML file, any sidecar asset, or a file over
  1.5 MB.
- A rendering convention: `document.createElement` and `textContent`. This
  bundle assigns `innerHTML` nowhere.

### Major constraints recorded here

- The guard must not be weakened, and its exclusion list must not grow to
  accommodate a dependency's convenience. That is stated in the engineering
  rules and was the deciding argument in ADR-0009.
- **`packages/editor` is bound by this decision.** It is bundled into the
  offline form, so whatever it exports must be usable without a framework
  runtime, must make no network call, and must contain no literal absolute URL.
- No comment in this bundle spells out an absolute URL either: the guard scans
  the built file, and a comment surviving an unminified build would trip it.

---

## Step 6 — LAN Server & Participant Join

**Status: COMPLETE** (`334cdf2`)

### Intent

Let people in the room join from a browser, without an account, without an
install, and without the browser ever being trusted about who it is.

### What was established

- **Embedded LAN UI.** `apps/lan-ui` is built to static assets and compiled into
  the Rust binary with `rust-embed`, so the participant page is served from the
  Host process with no files on disk. A guard asserts exactly one embedded
  folder and that it is the participant bundle — `host-ui` is never embedded.
- **Axum server.** `app_server::start` binds `0.0.0.0` on a Host-chosen port;
  `127.0.0.1` is never assumed reachable by participants. Security headers on
  every response: a CSP with no remote origin, `nosniff`, `no-referrer`,
  `DENY` framing. A 1 KiB body limit.
- **Join token.** 256 bits from the OS CSPRNG, rendered as 64 lowercase hex
  characters. Issued by the Host, carried in the join URL and QR.
- **Participant claim.** `GET /api/join/{token}` lists claimable identities;
  `POST /api/join/{token}/claim` binds one.
- **Session authentication.** The claim returns a session token once, to the
  browser that earned it. `GET /api/session` authenticates with
  `Authorization: Bearer`.
- **Actor construction at the transport edge.** Two extractors, and between them
  the only way a handler can obtain an `Actor`: `JoinContext` from the path
  token, `Participant` from the bearer token. Every field of
  `Actor::Participant` comes from the resolved session row.
- **Server-side authorization.** Every mutation goes through `Domain`. No
  handler decides whether the meeting is open, whether an identity is free, or
  whether a session may act.
- **Tauri LAN lifecycle commands.** `start_lan_server`, `stop_lan_server`,
  `lan_server_status`, `list_lan_interfaces`, `issue_join_token`,
  `approve_participant_claim`, `revoke_participant_session`. Opening a meeting
  does **not** start the server: binding a LAN-reachable socket is the Host's
  decision, not a side effect (ADR-0016). Stopping waits until the port is
  actually free.
- **Token hashing.** SHA-256; only the hash is stored. `app-core` accepts only a
  `TokenHash`, which a plaintext token will not parse as, so the secret cannot
  reach the database even by mistake. `Credential`'s `Debug` is hand-written to
  redact the plaintext.
- **First-claim-wins.** Enforced twice: read inside the transaction, and by the
  partial unique index that decides a race the read cannot see. A lost race and
  a plain second attempt look identical to the caller.
- **Host approval is acknowledgement, not an authorization gate.** A claim is
  usable the moment it exists; `approved_at` records only that the Host noticed.
  No authorization decision reads it, and a test asserts as much. `PENDING`
  therefore means *joined and working*, never *waiting for permission*.
- **Session-specific revocation.** Revoking sets `revoked_at`; the row stays as
  history and the identity becomes claimable again.
- **No credential leakage.** A refusal never carries an identifier, so an
  unknown join token and an unknown route return byte-identical 404s. SQLite
  diagnostics stay on the Host's console.
- ADR-0016 (the LAN session and claim model) and ADR-0017 (local QR generation).

---

## Step 7 — Realtime Updates & Participant Presence

**Status: COMPLETE** (`f5e1b1f`)

### Intent

Make both sides of the meeting update themselves, without the notification
channel ever becoming a second, weaker copy of the API.

### What was established

- **`DomainEvent`.** Thirteen variants, published by `Domain` **after** the
  transaction commits, never before. A refusal, a validation failure and a
  rollback emit nothing.
- **`Audience` and `EventSink`.** `DomainEvent::audience()` is an exhaustive
  match producing `HostOnly`, `Meeting`, `Participant` or `ParticipantSession`.
  Adding a variant does not compile until its audience is chosen.
  `EventSink::publish` returns nothing, so no client can affect a committed
  write. `CompositeSink` fans out.
- **`note.changed` and the rest of the event infrastructure.**
  `note.changed` exists with audience `Participant(meeting, participant)` and is
  already emitted by `Domain::write_note`. Step 8 consumes it rather than
  introducing anything.
- **WebSocket authentication using the subprotocol.**
  `new WebSocket(url, ['lan-meeting.v1', sessionToken])`. Exactly two entries,
  tag first, matched as a whole. The server echoes only the tag.
- **No query-string credentials.** A structural guard asserts the `/ws` route
  reads no identity from the URL and that the participant bundle puts no token
  in the socket URL.
- **Server → client application messages only.** There is no inbound
  application protocol. A `Text` or `Binary` frame from a client closes the
  socket with 1003. This eliminates a parsing and authorization surface on the
  transport that faces untrusted input.
- **Tauri Host event propagation.** The Host is **not** a WebSocket client. It
  is in the same process as the database and receives events over IPC through
  `TauriEventSink` on the event name `domain-event`.
- **Presence registry.** An in-memory count of open sockets per identity. Not a
  column: a Host machine that loses power would otherwise leave a database
  claiming everybody is still here.
- **0→1 / 1→0 persisted presence transitions.** `last_seen_at` is written on
  those transitions only, and means *the last moment a socket was observed to
  open or close*. A second tab cannot make a roster flicker.
- **No heartbeat database writes.** The 30-second keepalive is a protocol ping
  and writes nothing. Presence is not audited and grants no authority.
- **Session-specific revocation.** `session.revoked` carries the `session_id`
  from the row the transaction acted on; only sockets authenticated with that id
  close, with code 4401.
- **Reconnect requires full authentication and refetch.** No authorization state
  survives between sockets, and there is no event replay — a socket that has just
  opened is about to re-read current state anyway.
- **Locked sockets remain open while mutations are rejected.** Locking
  broadcasts `meeting.locked` and disconnects nobody. The backend refuses the
  edit regardless, because the status is re-read inside every mutating
  transaction.
- **Bounded broadcast and resync behaviour.** A 256-event bounded channel; a
  socket that falls behind is closed with 4408 and reconnects and refetches. No
  broker, no durable log, no acknowledgements, no per-client queues.
- ADR-0018.

---

## Step 8 — Host Note Editing & Version History

**Status: COMPLETE** (implemented, uncommitted at the time of writing)

This section is the implementation prompt Step 8 was executed from. It is kept
verbatim as the record of the approved scope; ADR-0019 records what was decided
and why, including the two places the implementation differs from the prompt:

- **Ordered lists were implemented** although D6's supported list omits them,
  because PRD section 13.2 and ADR-0007 both require them and those documents
  are authoritative over this file.
- **`@types/node` and `happy-dom` were added** as dev dependencies alongside
  Vitest (D4): the fixture loader needs Node types, and the renderer's DOM
  guarantee has to be asserted against a DOM rather than a stand-in.

### Scope

Step 8 implements **only**:

1. framework-neutral Markdown editor package,
2. strict GFM-subset parser / serializer / renderer,
3. safe DOM rendering,
4. link-scheme allowlist,
5. Rust note-content validation hardening,
6. `app-db` note read queries,
7. Tauri note read / write / version commands,
8. Host note UI,
9. Host note history UI,
10. Host realtime reaction to `note.changed`,
11. shared Markdown fixtures,
12. TypeScript tests,
13. ADR-0019,
14. documentation and roadmap updates needed to record Step 8B and deferred
    scope.

### Explicitly NOT in Step 8

Do **not** implement:

- participant note editing,
- LAN participant note API,
- note links CRUD,
- remote form note integration,
- note restoration,
- meeting lock command,
- PDF export,
- AI integration,
- Rust Markdown renderer,
- cloud features.

### Approved decisions

**D1 — Restore: DEFER.**
Version history is **view-only** in Step 8. Do not add `NoteRestored`, a restore
command, restore UI, or restore audit semantics.

**D2 — Structured note links: DEFER.**
The existing `note_links` schema must not be expanded into CRUD behaviour in
Step 8.

**D3 — Participant note editing: becomes its own step.**
Recorded below as **Step 8B — Participant Note Editing & LAN Note API**. Do not
implement Step 8B while implementing Step 8.

**D4 — TypeScript testing: APPROVED.**
Use **Vitest** as the TypeScript test runner. Add the minimum necessary root
configuration so the editor tests run through the project's normal validation
flow.

**D5 — Maximum note size: APPROVED, 64 KiB.**
A maximum of 64 KiB **UTF-8 encoded bytes**. The backend is authoritative. The
UI may show a character or size count as a convenience, but that must not
replace the byte-size validation.

**D6 — Markdown subset: APPROVED, with the raw-HTML rule corrected.**

Supported:

- paragraphs,
- ATX headings `#` through `######`,
- unordered lists using `-`,
- GFM pipe tables,
- Markdown links `[text](url)`,
- emphasis `*em*`,
- strong `**strong**`,
- inline code,
- fenced code,
- line breaks.

> **Correction applied during implementation — ordered lists are supported.**
> The list above is the prompt as written, and it omits ordered lists. PRD
> section 13.2 and ADR-0007 both require them, and both outrank this file, so
> `1.` lists are implemented, fixtured and tested. They are **not** in the
> unsupported list below and must not be read as excluded. See ADR-0019,
> decision 2.

Unsupported:

- images,
- raw HTML,
- blockquotes,
- task lists,
- strikethrough,
- footnotes,
- autolinks,
- reference links,
- nested lists beyond one level,
- setext headings.

**Raw HTML rule — important correction.** Do **not** reject ordinary
mathematical or textual use of `<`. `a < b` and `2 < 3` must remain valid plain
text. Reject only syntactically recognizable raw HTML constructs, such as
`<script>`, `</div>`, `<!-- comment -->`, `<?xml ...?>` and similar.

The TypeScript and Rust validators must agree on the shared fixture
expectations.

**D7 — DRAFT note mutability: KEEP CURRENT BEHAVIOR.**
The existing `Domain` implementation allows note writes in `DRAFT` and `OPEN`
and rejects them in `LOCKED`. Do not change this in Step 8.

**D8 — Concurrency: last-write-wins.**
Do not introduce `expected_version` optimistic concurrency in Step 8. Historical
versions preserve overwritten content. The Host UI may provide an **advisory**
dirty-state warning when `note.changed` arrives while the local editor has
unsaved changes.

**D9 — The existing `shipped_code` guard issue: separate concern.**
Do not broaden Step 8 into an unrelated guard refactor. However, new code must
not be placed in a way that bypasses existing production-code scanning or guard
assumptions.

### Editor requirements

`packages/editor` remains **framework-neutral**. Do not add React. Render with
DOM APIs.

Do not use:

- `innerHTML`,
- `dangerouslySetInnerHTML`,
- HTML-string rendering,
- `img`, `iframe`, `object`, `embed`, `script`, `style`,
- SVG rendering.

Use `textContent` for text nodes.

Links must:

- parse safely,
- use `new URL(...)` where appropriate,
- allow only `http:`, `https:` and `mailto:`,
- render non-allowlisted links as **plain text**,
- use `target="_blank"` only together with `rel="noopener noreferrer"`.

Do not put literal `http://` or `https://` URL strings in the editor
implementation: the remote-form offline guard must remain compatible.

The package should include appropriate parsing, validation, serialization,
rendering and scheme-allowlist modules.

### Markdown canonical behavior

- The stored note remains canonical Markdown.
- The editor's primary editing state is the **raw Markdown string**.
- Do not silently normalize user-authored Markdown during an ordinary save.
- Programmatic serialization may end with exactly one trailing newline where
  serialization is explicitly requested.

### Rust validation

Backend validation remains authoritative. Validate:

- non-empty content,
- UTF-8 byte size ≤ 64 KiB,
- prohibited raw HTML constructs (per D6),
- Markdown link target schemes,
- prohibited control characters.

Newline and tab are allowed. Malformed URLs must be handled safely.

### Database reads

Add read-side `HostQueries` for:

- current participant note,
- note version list,
- individual note version,
- notes overview for a meeting.

Version history queries must not fetch every historical body unnecessarily. Use
deterministic, explicit, total ordering.

### Tauri commands

Add only the commands Step 8 requires:

- get participant note,
- write participant note,
- list note versions,
- get note version.

Do **not** add restore commands. Host writes continue through the
`app-core` / `Domain` mutation boundary.

### Host UI

Add a Notes area or tab, with:

- participant list,
- note existence and version indicator,
- current note rendering,
- edit mode,
- textarea / editor,
- preview,
- save and cancel,
- character or size indication,
- version history,
- historical version preview.

When the meeting is `LOCKED`, editing controls are hidden or disabled, and the
backend remains authoritative and rejects writes regardless.

When `note.changed` arrives:

- clean editor → reload;
- dirty editor → preserve the draft and show an explicit conflict/update notice;
  the user can keep editing, or discard and reload.

Do not implement optimistic mutation behaviour.

### Realtime

The Host reacts to the existing `note.changed` event. Do not send note body
content over the WebSocket. Events carry identifiers and version metadata;
clients refetch note content through the normal read path.

### Shared fixtures

Create shared Markdown fixtures usable later by TypeScript editor tests, Rust
validation tests and a future Rust renderer. Include valid **and** invalid
cases, with explicit regression fixtures for:

- `a < b`,
- `2 < 3`,
- `<script>`,
- `</div>`,
- comments,
- malformed links,
- disallowed schemes,
- allowed `http` / `https` / `mailto`,
- size limits,
- control characters,
- supported Markdown constructs,
- unsupported constructs.

### Documentation

Create **ADR-0019** for Step 8. Update the implementation roadmap and
documentation so that Step 8B is explicitly recorded before Step 9. Do not
rewrite unrelated documentation.

### Validation

Before declaring Step 8 complete, run the applicable full validation suite:

- `cargo fmt`,
- `cargo check`,
- `cargo clippy`,
- `cargo test`,
- TypeScript typecheck,
- Vitest,
- frontend builds,
- remote-form offline guard,
- existing repository checks.

All existing tests must continue to pass. Do not weaken or delete tests to make
the implementation pass.

### Git

Do **not** commit. Do **not** push. Leave the working tree available for review.

---

## Step 8B — Participant Note Editing & LAN Note API

**Status: NEXT — NOT IMPLEMENTED**

Added as decision D3 during the Step 8 design review: PRD section 15 requires a
participant to be able to create and edit their own note during `OPEN`, and the
original roadmap had no step for it.

Intended future scope:

- participant note UI in `apps/lan-ui`,
- LAN note read and write endpoints,
- the authenticated participant actor, resolved as every other LAN request
  already resolves it,
- own-note authorization — a participant may write their own note and no other,
  which `authorize()` already enforces,
- realtime `note.changed` handling on the participant side,
- appropriate request and body limits (the current LAN body limit is 1 KiB and a
  note body will not fit in it),
- lock enforcement, re-read inside the mutating transaction as usual,
- the participant-side Markdown editor, reusing `packages/editor` unchanged.

What Step 8 already put in place for it:

- `packages/editor` is framework-neutral and needs no change to be used by
  `apps/lan-ui`;
- `authorize()` already permits `Actor::Participant` to write their own note
  and refuses every other one;
- `app-core::note` already validates content for whichever transport calls it;
- `note.changed` already reaches the note's owner over the participant socket.

What is missing is a route, a body limit larger than the current 1 KiB, and the
participant-side surface. A boundary guard asserts no such route exists yet, so
it cannot appear without the step being taken deliberately.

---

## Step 9 — Remote Form Generation

**Status: PLANNED**

From `README.md` roadmap item 9 and PRD sections 10 and 20.

Intended scope: generate the self-contained offline HTML form for a remote
participant, carrying immutable meeting and identity metadata, with no network
access of any kind and no credential inside it. `packages/editor` is bundled
here, which is why the editor must carry no literal absolute URL and no
framework runtime.

Details beyond the above are **TBD** and should be designed against the PRD and
ADR-0009 when the step is reached.

---

## Step 10 — Remote Submission Import Pipeline

**Status: PLANNED**

From `README.md` roadmap item 10 and PRD sections 11 and 12.

Intended scope: parse, validate and import a submission file as **untrusted
input**, in a single transaction, with duplicate detection backed by the
existing `remote_submissions` table. Import updates the participant's single
note and appends a version; it never creates a second note (ADR-0003).

Details beyond the above are **TBD**.

---

## Step 11 — Meeting Lock

**Status: PLANNED**

From `README.md` roadmap item 11 and PRD section 19.

Intended scope: expose the already-implemented `Domain::lock_meeting` through
the Host command surface, with the irreversibility made clear in the UI. The
domain rule, the audit record and the `meeting.locked` event already exist.

Details beyond the above are **TBD**.

---

## Step 12 — Export: Markdown and TXT / AI Context

**Status: PLANNED**

From `README.md` roadmap item 12, PRD sections 20 and 21, ADR-0004 and
architecture rules section 19.

Intended scope: deterministic renderers in `crates/app-export`. Export states the
meeting timezone explicitly, uses explicit total ordering everywhere, and
preserves meeting metadata, participants, notes, links, timestamps and relevant
audit information. AI Context only formats and structures — no summarization,
inference or interpretation, and no AI API anywhere. **PDF is Phase 2 by
decision and no PDF dependency may be added during Phase 1.**

This is also where the Rust Markdown renderer lands, and where the shared
fixtures created in Step 8 get their second consumer, satisfying ADR-0007's
requirement that the TypeScript and Rust renderers agree on the same subset.

Details beyond the above are **TBD**.

---

## Current Execution Point

> Phase 1 is complete through Step 8, which is implemented and awaiting review
> in the working tree. Step 8B — Participant Note Editing & LAN Note API — is
> the next implementation step and must be executed separately.

Repository state:

- branch `main`, `HEAD` = `f5e1b1fd54efde089da7fc2413f0ef7f2bdafdb2`
- Step 8 changes are in the working tree, uncommitted, awaiting review
- Rust and TypeScript suites both passing; Vitest arrived with Step 8 (D4)
