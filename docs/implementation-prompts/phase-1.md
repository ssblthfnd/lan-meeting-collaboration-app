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
| Step 8 | Host note editing & version history | COMPLETE | `27349ec` |
| Step 8B | Participant note editing over LAN | COMPLETE | `120a9c5` |
| Step 9 | Remote form generation | COMPLETE | `e867bdc` |
| Step 10 | Remote submission import pipeline | COMPLETE | `4d646b6` |
| Step 11 | Meeting lock | COMPLETE | `a9ef7f8` |
| Step 12 | Export: Markdown and TXT / AI Context | COMPLETE | uncommitted |

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

**Status: COMPLETE** (`27349ec`)

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

## Step 8B — Participant Note Editing over LAN

**Status: COMPLETE** (`120a9c5`)

Everything below is the approved design, and it was implemented as written:
every decision D1-D7 stands unchanged, `app-core` was not touched, and no
migration was created. ADR-0020 records the implementation. The guard that
asserted this step had not happened was replaced by five that assert the shape
it took.

Added as decision D3 during the Step 8 design review: PRD section 15 requires a
participant to be able to create and edit their own note, and the original
roadmap had no step for it.

### Approved architecture

```text
apps/lan-ui
  └─ authenticated LAN HTTP API
       └─ existing Participant extractor
            └─ Actor::Participant { meeting_id, participant_id, session_id }
                 └─ existing Domain::write_note
                      └─ SQLite transaction
                           notes + note_versions + audit_logs
                         COMMIT
                      └─ existing note.changed
                           ├─ participant WebSocket (own sockets only)
                           └─ Host Tauri event
```

**No `app-core` or domain redesign is required.** `authorize()` already approves
`Actor::Participant` for `Operation::WriteNote { participant_id }` when the
target participant id equals the actor's own, and refuses everything else by
default. The domain remains the final authorization boundary.

### Approved route design

```text
GET /api/note      -> the participant's own note, or null
PUT /api/note      -> replace it
```

**Not** `/api/meetings/{meeting_id}/me/note`.

The meeting and the participant are already established by the authenticated
session, so the route accepts **no participant id and no meeting id**. That
makes identity substitution structurally inexpressible at the HTTP layer rather
than merely checked, and it matches the principle `/api/session` already
follows: a path with nothing on it is a path with nothing to forge.

The browser must never send participant identity as an authority-bearing
parameter.

### Approved `GET` behaviour

- Authentication: the existing `Participant` extractor.
- Identity source: the authenticated session row.
- Query scope: `meeting_id` + `participant_id`, both from the actor.
- Response: the note DTO when one exists, `null` before the participant has
  written anything.
- **`GET` remains available when the meeting is `LOCKED`.** A locked meeting
  prevents mutation, not reading.

### Approved `PUT` behaviour

Request body, in full:

```json
{ "content": "..." }
```

It must **not** carry `participant_id`, `meeting_id`, `note_id`, `session_id` or
`expected_version`. Identity is derived from the session.

The write uses the existing `Domain::write_note` with unchanged semantics:
authorization checked by the domain, the lock re-read **inside** the
transaction, Markdown validated inside the transaction, the single note
upserted, a version appended, an audit entry appended, commit, and only then
`note.changed`.

Last-write-wins remains the established behaviour. **No optimistic concurrency
is introduced.**

### Approved participant note DTO

Carries: `content`, `version`, `updated_at`, `last_author_type`.

Does **not** carry: `note_id`, `last_author_id`, any Host-only note metadata, or
version history.

`last_author_type` is retained so a participant can tell whether their current
note was last changed by themselves, by the Host, or by another approved
mutation source such as a future remote import. Participant version history
remains Host-only (PRD section 17 assigns history to the Host).

### Approved database read model

Add a dedicated `ParticipantQueries::own_note(..)`.

Do **not** reuse `HostQueries::note(..)`: read models are audience-specific
under ADR-0014, and the Host's shape carries fields a participant has no need
for. The participant query returns only the DTO's fields.

**No migration is required.**

### Approved realtime behaviour

No WebSocket architecture change. `DomainEvent::NoteChanged` and its audience
routing stay exactly as Step 7 and Step 8 left them.

The event carries metadata only — `meeting_id`, `participant_id`, `note_id`,
`version`, timestamp — and **never note content**. A participant receives
`note.changed` for their own note and for no other; the existing session-scoped
audience filtering remains authoritative.

Participant editor behaviour, mirroring the Host:

- **clean editor** — refetch the note;
- **dirty editor** — preserve the local draft, show a conflict/update notice,
  and offer *Keep editing* or *Discard mine and reload*.

No optimistic concurrency, and no `expected_version` is ever sent. A
participant's own save echo may be ignored when the event's version is already
represented by the locally known saved version; that is client-side display
logic over a value the server returned, not a concurrency control.

### Approved body-limit strategy

The global LAN body limit stays at **1024 bytes**, unchanged, for every other
route and the asset fallback.

A route-scoped override applies to `/api/note` only, with a transport cap of
**192 KiB**.

The authoritative note-size rule remains **64 KiB of UTF-8 bytes**, enforced in
`app-core::note`. The two layers are deliberately different things:

| Layer | Purpose |
| --- | --- |
| 192 KiB route cap | memory and request protection |
| 64 KiB domain rule | the actual note-size rule |

So a payload between 64 KiB and 192 KiB reaches the domain validator and is
refused by the 64 KiB rule with a message naming both the limit and the measured
size. A payload above the route cap is refused by the transport with HTTP 413.

### Approved error behaviour

- Add `ApiError::payload_too_large()` and its LAN error-contract representation
  for HTTP 413.
- Existing validation refusals remain HTTP 400.
- A `LOCKED` mutation maps through the existing meeting-lock convention.
- Malformed JSON should be mapped consistently as `invalid_request` where the
  existing route infrastructure permits it.

**`routes::claim` is explicitly out of scope.** Its pre-existing malformed-JSON
behaviour must not be modified in Step 8B.

### Approved participant UI

Reuse `packages/editor` **unchanged**. Do not create a second Markdown parser,
serializer, validator or renderer.

The own-note panel provides: the rendered note, *Edit*, a textarea, a live
preview, *Save*, *Cancel*, a character count, a byte count, validation and save
error state, last-changed information, and a locked read-only state.

Locked state is derived from the meeting/session state already loaded. Backend
enforcement remains authoritative; the read-only UI is presentation.

### Approved note-links decision

Participant note links remain **DEFERRED**.

PRD section 5.2 permits a participant up to five links, but the note-link
schema and behaviour are not sufficiently resolved for Step 8B — in particular
`note_versions` versions content only, and what a version means for links is
still open (ADR-0019, decision 9).

This stays an explicitly tracked future item. **The existing ADR must not be
altered merely to remove the requirement.**

### Approved audit and version semantics

A participant write uses the existing mutation path and atomically creates:

1. the note row,
2. a `note_versions` row,
3. an `audit_logs` entry.

With `created_by_type = PARTICIPANT` and `created_by = <participant id>`, which
the schema's `CHECK ((created_by_type = 'HOST') = (created_by IS NULL))` already
requires.

Audit metadata must not contain note body content. `note.changed` is emitted
only after a successful commit; a failed mutation creates neither a version nor
an audit entry.

### Approved lifecycle behaviour

Under the current claim flow a participant session can only exist for an `OPEN`
meeting, because `claim_identity` calls `ensure_open()` and no transition
returns a meeting to `DRAFT`. Participants therefore edit during `OPEN`,
`LOCKED` rejects mutation at the backend, and `GET` remains available after the
lock. **No lifecycle change is required.**

### Approved security constraints

Step 8B must preserve all of these:

- no participant id from the request body;
- no participant id from the URL;
- no meeting id from the request body;
- no meeting id from the URL;
- no session credential in a query string;
- revoked sessions cannot authenticate;
- a participant cannot access another participant's note;
- a participant cannot mutate another participant's note;
- no note body in WebSocket events;
- no global body-limit increase;
- no direct database access from `app-server`;
- no bypass of the `app-core` mutation boundary;
- no validation bypass;
- lock enforcement inside the transaction;
- no unsafe HTML rendering;
- the shared editor implementation only.

### Expected implementation file plan

Change only what the implementation actually requires, from this list:

```text
crates/app-db/src/participant_query.rs
crates/app-db/src/lib.rs
crates/app-server/src/dto.rs
crates/app-server/src/routes.rs
crates/app-server/src/router.rs
crates/app-server/src/error.rs
crates/app-server/src/state.rs        (only if actually required)
crates/app-server/tests/http_contract.rs
crates/app-server/tests/realtime_socket.rs
crates/app-db/tests/notes.rs
packages/contracts/src/lan.ts
apps/lan-ui/package.json
apps/lan-ui/src/api/lanApi.ts
apps/lan-ui/src/components/Markdown.tsx
apps/lan-ui/src/components/NotePanel.tsx
apps/lan-ui/src/components/JoinedView.tsx
apps/lan-ui/src/App.tsx
apps/lan-ui/src/styles.css
src-tauri/tests/boundaries.rs
docs/adr/0020-participant-note-editing.md
```

### Expected untouched areas

```text
crates/app-core/**
crates/app-db/migrations/**
crates/app-db/src/query.rs
crates/app-export/**
crates/app-remote/**
src-tauri/src/**
packages/editor/**
apps/host-ui/**
apps/remote-form/**
scripts/check-remote-form-offline.mjs
docs/adr/0001 - 0019
```

If implementation reveals that one of these must change, **stop and report it as
an unresolved design issue** rather than silently expanding scope.

### Approved test scope

**Domain and database.** Participant can write their own note; cannot write
another participant's; cannot act in another meeting; a revoked session cannot
write; `OPEN` allows the write; `LOCKED` rejects it; invalid Markdown is
rejected; over 64 KiB is rejected; participant version metadata is correct; a
successful write creates the version and the audit entry atomically; a failed
write creates neither.

**LAN HTTP.** Authenticated `GET` of the own note; `GET` returns null before the
first write; authenticated `PUT` creates; authenticated `PUT` updates;
unauthenticated `GET`/`PUT` rejected; revoked session rejected; a participant
cannot substitute another identity; another participant can neither read nor
write this note; `GET` works while `LOCKED`; `PUT` fails while `LOCKED`; the
global 1 KiB limit still applies to other routes; the note route accepts
legitimate payloads below 64 KiB; a payload above 64 KiB reaches the
authoritative validator; a payload above 192 KiB is refused by the transport; an
unauthenticated oversized request never becomes an authenticated note operation;
CRLF normalisation remains correct.

**Realtime.** An own-note change emits `note.changed`; the event carries no note
body; another participant receives no unauthorized event or content; revoked
live-session behaviour is unchanged; a locked mutation emits no success event.

**UI.** A participant can read, edit, save, cancel and preview their own note; a
clean editor reacts to a remote change; a dirty editor preserves the draft and
offers reload/discard; a locked editor is read-only; validation and save errors
are visible; Markdown rendering uses the shared safe renderer.

**Guards.** Preserve every existing boundary and security guard; no HTML sink;
no duplicate Markdown renderer; no participant history route; no participant
identity parameter in the note route; the offline remote-form guard and the
`shipped_code` guard remain unchanged.

### Explicitly not part of Step 8B

Participant note-links CRUD, participant note history, restore, remote-form note
integration, remote import changes, Markdown/TXT/AI Context export, PDF, AI
integration, the meeting lock command, optimistic concurrency,
`expected_version`, any WebSocket redesign, any authentication redesign, cloud
functionality, Step 9, and any unrelated cleanup.

### Design decisions — all resolved

| # | Decision | Outcome |
| --- | --- | --- |
| D1 | Route shape `/api/note` with no identifiers | **APPROVED** |
| D2 | `ApiError::payload_too_large()` / HTTP 413 | **APPROVED** |
| D3 | Include `last_author_type` in the participant DTO | **APPROVED** |
| D4 | Participant note links | **DEFERRED** |
| D5 | `routes::claim` malformed-JSON inconsistency | **OUT OF SCOPE** |
| D6 | 192 KiB route-level transport cap | **APPROVED** |
| D7 | `GET` the note while `LOCKED` | **APPROVED** |

No design blocker remains. If source inspection during implementation uncovers a
contradiction with the authoritative architecture, stop and report it rather
than resolving it by weakening a higher-authority requirement.

### What Step 8 already put in place

- `packages/editor` is framework-neutral and needs no change to be used by
  `apps/lan-ui`;
- `authorize()` already permits `Actor::Participant` to write their own note and
  refuses every other one;
- `app-core::note` already validates content for whichever transport calls it;
- `note.changed` already reaches the note's owner, and only the owner, over the
  participant socket.

What is missing is the route, the route-scoped body limit, the participant read
model, the DTOs and the participant-side surface.

---

## Step 9 — Remote Form Generation

**Status: COMPLETE** (`e867bdc`)

From `README.md` roadmap item 9, PRD sections 10, 11 and 20, and architecture
rules sections 6, 7, 8 and 20. The accepted decision record is
`docs/adr/0021-remote-form-generation.md`.

Everything below is the approved design, and it was implemented as written:
every decision D1-D15 stands unchanged, `app-core` and the migrations were not
touched, and no migration was created. Three points are worth recording because
they were open when the design was frozen:

- **D6 was verified empirically and the primary mechanism held.** Vite with
  `vite-plugin-singlefile` preserves a non-module
  `<script type="application/json" id="submission-context">` through the build,
  byte for byte, and the offline guard still passes. The string-literal sentinel
  fallback was **not** needed and was not built.
- **The canonical form is a length-prefixed field encoding, not JSON.** D12
  asked for "the hashed fields in deterministic declaration order" and did not
  name a format. Two JSON serialisers agreeing byte for byte on every escape is
  an assumption the contract does not need to make, and the note - the one
  arbitrary-text field - is what decides the hash. Length-prefixing it removes
  the question entirely.
- **Generation is not audited.** It mutates nothing: no note, no version, no
  audit row, no ledger row. Writing an audit entry would need a new
  `Operation` in `authz.rs`, which the frozen plan lists as untouched. The
  import *is* audited, and that is step 10.

A read-only design inspection of the repository at `120a9c5` preceded these
decisions. If implementation uncovers a contradiction with the authoritative
architecture, stop and report it rather than resolving it by weakening a
higher-authority requirement.

### What already exists, verified at `120a9c5`

Step 9 is unusually well prepared, and knowing that prevents rebuilding it:

- `remote_submissions` exists in migration V1 with `submission_id`,
  `content_hash`, `resolution`, `note_version`, `raw_payload`, the composite
  foreign key to `participants (meeting_id, id)` and
  `UNIQUE(meeting_id, participant_id, submission_id, content_hash)`. **No Rust
  code reads or writes it yet.**
- `Actor::RemoteImport { meeting_id, participant_id }` exists, reports
  `REMOTE_IMPORT`, and `authorize()` grants it exactly
  `Operation::WriteNote { participant_id }` for its own participant. Four tests
  assert the confinement. **No code constructs it.**
- `note_versions.created_by_type` already accepts `REMOTE_IMPORT`.
- `SubmissionId`, `RemoteSubmissionId`, `SUBMISSION_SCHEMA_VERSION` and
  `SubmissionResolution` all exist.
- `packages/contracts/src/index.ts` already documents `form-payload.ts` and
  `submission.v1.ts` as files that should exist. **They do not.**
- `crates/app-remote` is a 44-line skeleton: `RemoteError` has two variants,
  `Schema` and `MeetingMismatch`, and nothing else exists.
- `apps/remote-form` renders four paragraphs. Its offline build guard passes at
  1.39 kB and **must not be weakened**.
- `DbError::is_constraint_violation()` exists and its documentation already
  names duplicate remote submissions as its reason.

### Approved architecture

```text
apps/remote-form  (npm build + offline guard)
  └─ dist/index.html
       └─ rust-embed, via crates/app-remote/build.rs
            └─ app-remote::generate(context) -> String
                 └─ src-tauri command
                      └─ std::fs write into the application data directory
                           └─ Host UI shows the path and the submission id
```

The participant's side, entirely offline:

```text
file:// form
  └─ JSON island  ->  form state (identity read-only, note editable, prefilled)
       └─ packages/editor validation
            └─ Export Submission  ->  Blob download, or copy-to-clipboard fallback
                 └─ submission JSON, carried back by whatever channel the Host chose
```

### Frozen decisions

| # | Decision | Outcome |
| --- | --- | --- |
| D1 | Structured `links` in the form and the submission | **OMITTED** |
| D2 | Import lifecycle rule | **`ensure_mutable()`** — DRAFT and OPEN permitted, LOCKED rejected |
| D3 | A newer form invalidating older ones | **NO** — older forms stay importable |
| D4 | Prefill the form with the participant's current note | **APPROVED** |
| D5 | Persist rejected submissions in the ledger | **NO** for the MVP |
| D6 | Payload injection via an escaped JSON island | **APPROVED**, verified empirically at implementation time |
| D7 | Blob download plus an offline copy fallback | **APPROVED** |
| D8 | Cross-language submission fixtures | **APPROVED** |
| D9 | `source_version` | **ADVISORY**, no database column |
| D10 | Host filesystem access | **`std::fs` only**, no plugin, no capability change |
| D11 | Submission artifact identity and duplicate semantics | **FROZEN** — see below |
| D12 | Canonical hash | **FROZEN** — see below |
| D13 | Cryptographic signing | **NONE** |
| D14 | Where `Actor::RemoteImport` is constructed | **`src-tauri` only** |
| D15 | Step 9 / Step 10 boundary | **FROZEN** — see below |

### D1 — no structured links

The submission carries `note` as GFM-subset Markdown and no `links` array. The
form offers no link fields. `note_links` stays untouched and
`boundaries.rs::note_links_are_still_only_a_schema` stays active.

Reason: ADR-0019 decision 9 and ADR-0020 both deferred structured links, for the
same unanswered question — `note_versions` versions content only. Introducing
links through the remote form would reopen two accepted decisions from the side
door. Markdown already carries links under the same three-scheme allowlist.

Architecture rules section 8 lists `links` in the minimal submission shape and
section 9 requires `link count <= 5`. ADR-0021 decision 6 records the
reconciliation: with no array there is nothing to count, the five-link limit
remains PRD section 14's rule and remains enforced by the V1 triggers, and the
section 9 checklist item is **not applicable** to a submission until structured
links are built. A parser must not invent a field in order to validate it.

### D2 — the lifecycle rule is the domain's

Remote import uses `Meeting::ensure_mutable()` unchanged:

```text
DRAFT   permitted
OPEN    permitted
LOCKED  rejected, inside the mutating transaction
```

No transport-specific lifecycle rule. Architecture rules section 9's "meeting
OPEN?" is read as "the meeting still accepts mutations", which is what
`ensure_mutable` means and what the LAN route and the Host command already
enforce. A stricter import-only rule would make three write paths disagree about
when a note may be written.

### D3 — older forms stay valid

Each generated form carries its own UUIDv7 `submission_id`. Generating a newer
form does not invalidate an older one; an older submission remains importable
until the meeting is locked or the participant leaves the roster.

**No revocation table, no expiry column, no invalidation flag, no migration.**

### D4 — the form is prefilled

The generation context carries:

```text
schema_version
submission_id            minted by the Host, UUIDv7
meeting_id, meeting_title, meeting_date, meeting_timezone
participant_id, participant_name
source_version           the note version current at generation, 0 if none
existing_content         the participant's current note, or empty
generated_at
```

Identity and meeting metadata are **read-only** in the form (PRD section 10).
`existing_content` is editable form state. It is informational only: it is never
an authorization source, never an identity source, and never evidence at import
time. Without it every remote submission would be a silent rewrite of whatever
the participant or the Host had already written.

### D5 — rejections are reported, not persisted

| Outcome | Ledger row |
| --- | --- |
| malformed or unparseable | none |
| schema-invalid | none |
| unknown meeting or participant | none |
| locked meeting | none |
| invalid note content | none |
| duplicate of an imported submission | none — reported from the **existing** row |
| successful import | one row, atomic with the note, version and audit rows |

No second rejection-write transaction. `REJECTED_DUPLICATE` and
`REJECTED_INVALID` stay in the schema's CHECK and are **not written** in the
MVP; using them means revisiting the ledger schema deliberately, in a later
design.

Two structural reasons, both verified in the V1 schema: a rejection row written
inside the import transaction would roll back with it, and the composite foreign
key to `participants (meeting_id, id)` makes a row impossible for exactly the
rejections that would matter most — an unknown meeting or an unknown
participant.

### D6 — the JSON island

```html
<script type="application/json" id="submission-context">null</script>
```

Read with `textContent` and `JSON.parse`. **Never `innerHTML`** — this bundle
has no HTML sink (ADR-0009).

Escaping is mandatory and is part of the decision, not an implementation
detail: `<` as a unicode escape, and likewise U+2028 and U+2029. Escaping `<`
unconditionally is what makes a closing script tag unspellable inside the
island. A test with a hostile participant name and a hostile note is required.

Whether Vite with `vite-plugin-singlefile` preserves a non-module
`<script type="application/json">` is an **expectation, not a verified fact**.
Verify it empirically during implementation. If it does not survive reliably,
use the documented string-literal sentinel fallback, which survives minification
because a string literal is not a comment.

**The offline guard is not weakened for either mechanism.**

### D7 — export, and its fallback

*Export Submission* builds the JSON, wraps it in a `Blob`, and offers it through
`URL.createObjectURL` and an anchor with `download`. A visible fallback ships
alongside: a read-only textarea with the same JSON and copy-to-clipboard, because
blob downloads from a `file://` origin behave inconsistently across browsers.

Both paths are entirely offline. No automatic submission, no upload, no network
transmission of any kind. The submission leaves the browser only when the
participant asks for it (architecture rules section 20).

### D8 — shared fixtures

`packages/contracts/__fixtures__/submissions.json`, with `valid` and `invalid`
groups, each invalid row naming its rejection reason. Read **at run time** by a
Rust test in `app-remote` and by a TypeScript test, the way
`packages/editor/__fixtures__/notes.json` is already read by both suites, so a
change cannot be applied to one language only.

Widen `vitest.config.ts`'s `include` only as far as this requires.

### D9 — `source_version` is advisory

Meaning: the note version current on the Host when the form was generated; `0`
when no note existed. Displayed in the import preview. **Never a concurrency
precondition**, never a comparison that blocks an import.

No database column, and **no migration**. It is preserved verbatim in
`remote_submissions.raw_payload` and may be written into `audit_logs.metadata`.
This is the answer ADR-0019 decision 10 deferred when it declined
`expected_version` "because it would need a considered answer for remote import,
which is deliberately last-write-wins".

### D10 — the Host's filesystem access

Generation: `src-tauri` writes with `std::fs` into the application data
directory, the command returns the exact path, and the Host UI displays the path
and the minted `submission_id`.

**No `tauri-plugin-dialog`, no `tauri-plugin-fs`, and no change to
`src-tauri/capabilities/default.json`.** The existing guard
`the_window_capability_grants_nothing_beyond_the_core_defaults` stands.

Import (step 10): investigate Tauri's core drag-and-drop path delivery, which
would hand Rust a path to read with `std::fs` and may need no plugin. If native
path delivery proves insufficient, fall back to pasting the submission JSON. A
filesystem plugin is **not** added for file-picker convenience; if the approach
proves impossible, that is a new architectural decision to be approved
explicitly.

### D11 — submission artifact identity

`submission_id` is a UUIDv7 **minted by the Host at form generation**, baked
into the HTML, and returned by the form unchanged. The form never mints one. It
identifies **a generated artifact**.

| Case | Meaning | Outcome |
| --- | --- | --- |
| same `submission_id`, same `content_hash` | the identical file again | duplicate; refused; **no second note version** |
| same `submission_id`, different `content_hash` | the artifact was altered after generation | **refused**, with an actionable error |
| different `submission_id` | a different generated artifact | imported on its own terms |

**This supersedes ADR-0008**, which called case 2 "a correction from the same
participant", and the same sentence in architecture rules section 12. The
mechanism ADR-0008 specified is unchanged; what changes is the verdict. An
artifact's content is fixed the moment the participant exports it, so a file
bearing a known `submission_id` with different content is a file that does not
match the one thing the Host can check.

**Corrections still work, through regeneration.** A participant who needs to
resubmit asks the Host for a new form, which carries a new `submission_id` and
imports under case 3.

Consequence: once a `submission_id` has been imported, nothing else bearing it
can ever be imported. Detection is a lookup on
`(meeting_id, participant_id, submission_id)`, already indexed by
`idx_remote_submissions_identity`; the `UNIQUE(...)` constraint remains the
backstop for the exact-duplicate case.

### D12 — the canonical hash

Computed by the Host at import time, never by the form, over a canonical form of
the submission rather than over the file's bytes:

1. parse into the typed submission structure;
2. normalise the note's line endings — CRLF to LF, lone CR to LF;
3. serialise the hashed fields in deterministic declaration order;
4. SHA-256 over those UTF-8 bytes;
5. store lowercase hexadecimal.

| Hashed | Excluded |
| --- | --- |
| `schema_version` | `generated_at` |
| `submission_id` | `submitted_at` |
| `meeting_id` | `participant_name` |
| `participant_id` | `source_version` |
| normalised `note` | |

Step 2 is the same normalisation `app-server::routes::write_note` already
applies at the LAN boundary, so a CRLF-mangled file and a clean one hash alike.
Hashing raw file bytes would refuse an artifact for a reformatting nobody
intended. `submitted_at` comes from an untrusted remote clock; including it would
make every export of an unchanged note a different artifact.

`content_hash` serves duplicate detection, artifact-modification detection and
idempotency. **It is not an authenticity mechanism.**

`sha2` is required by `app-remote`. It is already a workspace dependency and
already sanctioned by ADR-0006, so no new crate enters the tree.

### D13 — no cryptographic signing

No signing, no HMAC, no embedded secret, no join token, no session token, no
token hash, and no credential of any kind inside the offline form.

A signing key inside a file that runs offline from `file://` is a key that
everyone holding the file holds, so the signature would prove only what
possession of the file already proves. The offline artifact is **intentionally
not an authenticated credential**.

The trust boundary is unchanged and is stated in PRD section 12:

```text
database resolution  +  Host preview and confirmation  +  app-core authorization
```

Stated plainly, because it should be known rather than discovered: a hand-edited
file naming a different valid participant of the same meeting will import into
that participant's note if the Host confirms without reading the preview. The
preview must therefore show the participant name prominently, and import must
never be one click.

### D14 — where the remote actor comes from

`Actor::RemoteImport { meeting_id, participant_id }` is kept as it is. **Only
`src-tauri` may construct it.**

`crates/app-remote` parses and validates untrusted submission data and **must
not** construct it: the crate that reads the file is not the crate that produces
authority. A boundary guard asserts this.

The participant id inside a submission is a **candidate identifier**. The actor
is constructed from the **Host-selected meeting** and the **database-resolved
participant**, after validation and after Host confirmation. `participant_name`
is displayed for human verification and is never a match key (architecture rules
sections 10 and 21).

### D15 — the Step 9 / Step 10 boundary

**Step 9 — Remote Form Generation**

- form payload contract
- submission v1 contract
- the remote form UI
- existing-note prefill
- local validation, through `packages/editor`
- offline export, with the fallback
- HTML generation and template embedding
- the Host generation command
- minimal Host generation UI
- cross-language fixtures
- offline and security guards
- ADR-0021

**Step 10 — Remote Submission Import** (see its own section)

**Step 11 remains Meeting Lock.**

### Expected implementation file plan

Change only what the implementation actually requires, from this list:

```text
crates/app-remote/Cargo.toml
crates/app-remote/build.rs
crates/app-remote/src/lib.rs
crates/app-remote/src/context.rs
crates/app-remote/src/generate.rs
crates/app-remote/src/submission.rs
crates/app-remote/tests/generation.rs
crates/app-remote/tests/fixtures.rs
packages/contracts/src/form-payload.ts
packages/contracts/src/submission.v1.ts
packages/contracts/src/index.ts
packages/contracts/__fixtures__/submissions.json
apps/remote-form/package.json
apps/remote-form/index.html
apps/remote-form/src/app.ts
apps/remote-form/src/context.ts
apps/remote-form/src/form.ts
apps/remote-form/src/export.ts
apps/remote-form/src/styles.css
src-tauri/src/commands.rs
src-tauri/src/dto.rs
src-tauri/src/error.rs
src-tauri/src/lib.rs
apps/host-ui/src/api/hostApi.ts
apps/host-ui/src/components/RemoteFormPanel.tsx
apps/host-ui/src/components/ParticipantRoster.tsx
apps/host-ui/src/styles.css
src-tauri/tests/boundaries.rs
src-tauri/tests/remote_commands.rs
vitest.config.ts
package-lock.json
docs/adr/0021-remote-form-generation.md
```

### Expected untouched areas

```text
crates/app-db/migrations/**
crates/app-core/src/authz.rs
crates/app-core/src/actor.rs
crates/app-core/src/note.rs
crates/app-server/**
apps/lan-ui/**
packages/editor/**
crates/app-export/**
scripts/check-remote-form-offline.mjs
src-tauri/capabilities/default.json
docs/adr/0001 - 0020
```

If implementation reveals that one of these must change, **stop and report it as
an unresolved design issue** rather than silently expanding scope.

### Approved test scope

**Unit, `app-remote`.** Canonical serialisation is byte-stable; the hash ignores
key order, whitespace and `submitted_at`; CRLF and LF payloads hash identically;
one changed character changes the hash; a wrong `schema_version` is refused with
expected and detected named; malformed JSON, a missing field and a malformed id
are each refused; **template injection is escaped** for a hostile participant
name and for the two unicode line separators; a generated form round-trips its
payload; a generated form contains no token, hash, join URL or bearer
credential.

**Command level, `src-tauri`.** Generation returns a complete offline HTML
string and an exact path; generation for an unknown participant is refused
actionably; the file written is readable and self-contained.

**Guards.** Command count and gateway reachability updated; the capability set
unchanged; `app-remote` executes no SQL; `app-remote` never constructs
`Actor::RemoteImport`; the generated artifact carries no credential and matches
the offline guard's forbidden-pattern list; the remote form makes no network
call and sends nothing automatically; `note_links` remain schema-only; the
offline guard's forbidden list has not shrunk.

**TypeScript.** The shared submission fixtures agree with Rust row by row; the
form produces a schema-valid submission; the form refuses invalid content using
`packages/editor`; read-only identity fields are not editable.

### Explicitly not part of Step 9

Structured note links; note merge algorithms; optimistic concurrency or
`expected_version`; form invalidation, revocation or expiry; cryptographic
signing; cloud or network submission; automatic upload; filesystem or dialog
plugins; any new backend or API; PDF; AI; bulk form generation; bulk import; a
submission history UI; any change to the LAN transport; any change to
`packages/editor`'s validation rules; any database migration; and the whole of
Step 10.

---

## Step 10 — Remote Submission Import Pipeline

**Status: COMPLETE** (implemented, uncommitted at the time of writing)

Everything below is the approved design, and it was implemented as written:
D2, D5, D9, D11, D12, D13, D14 and O-1 through O-7 all stand unchanged, the
migrations were not touched, and no capability or plugin was added. Four points
are worth recording because they were settled during implementation:

- **The shared helper landed as two private functions** in `app-core::service`:
  a gate (`authorize_note_write`) and a write (`apply_note_write`).
  `Domain::write_note` was refactored onto both, so the authorization sequence,
  lock enforcement, content validation, the note upsert and version derivation
  exist in exactly one implementation. An import runs its artefact-identity
  lookups between the two, which is what keeps the gate's decisions ahead of
  them. Audit stays with each caller, so an import records
  `remote_submission.imported` rather than `note.updated`.
- **`DomainError` gained three variants, not two.** The cross-participant rule
  needed its own: `CrossParticipantArtifact` carries the participant the
  artefact was first imported for, which is what makes the refusal actionable.
- **`app-db` does not depend on `app-remote`.** The domain treats
  `content_hash` as an opaque 64-character string, so the database tests use a
  deterministic stand-in with the one property that matters - it changes when
  the participant changes. The real canonical hash is exercised where it
  belongs: by the shared fixtures, and end to end by the Tauri suite.
- **The LAN error contract gained an arm.** `app-server` must match
  `DomainError` exhaustively, and the three import refusals cannot be provoked
  from the LAN - there is no import route. They map to an internal failure with
  a diagnostic, because reaching one there would mean the application is wired
  wrong.

From `README.md` roadmap item 10, PRD sections 11 and 12, and architecture rules
sections 9, 10, 11 and 12. The accepted decision record is
`docs/adr/0022-remote-submission-import.md`.

Scope: parse, validate and import a submission file as **untrusted input**, in a
single transaction, with duplicate detection backed by the existing
`remote_submissions` table. Import updates the participant's single note and
appends a version; it never creates a second note (ADR-0003).

A read-only design inspection of the repository at `e867bdc` preceded this
freeze. No contradiction with the authoritative architecture was found during
implementation.

### What the inspection verified at `e867bdc`

Three problems that looked hard were already solved, and knowing that prevents
rebuilding them:

- **File import needs no capability change.** `core:default` resolves to
  `core:event:default`, which grants `allow-listen`; `dragDropEnabled` is on by
  Tauri's default and is not disabled in `tauri.conf.json`; and a Rust-side
  drag-drop handler needs no permission at all, because the ACL governs what the
  *renderer* may reach over IPC.
- **Cross-meeting import is structurally impossible.** `participants.id` is a
  global `PRIMARY KEY`, so a participant id belongs to exactly one meeting. The
  composite foreign key on `remote_submissions` is a second backstop.
- **Import attribution is automatic.** `DomainTx::insert_note_version` and
  `insert_audit` already take `actor_type` and `actor_id` from the authorization
  proof, so an `Actor::RemoteImport` proof writes correct `REMOTE_IMPORT`
  history and audit rows with **no change to either method**.

Also verified: `Db::write_with` opens `BEGIN IMMEDIATE`, commits on `Ok` and
drops the transaction on `Err`, behind a single writer `Mutex` - which is the
atomicity and serialisation mechanism this step needs, already built. `authz.rs`
has granted `Actor::RemoteImport` exactly `WriteNote { participant_id }` for its
own participant since step 2, with four tests. Step 9 already shipped
`SubmissionV1::parse`, `canonical_form` and `content_hash`, fixture-locked
across both languages.

### Frozen decisions

D2, D5, D9, D11, D12, D13 and D14 were frozen during the Step 9 design freeze and
were re-verified against source at `e867bdc` with **no contradiction found**.
O-1 through O-7 were resolved at this freeze.

| # | Decision | Outcome |
| --- | --- | --- |
| D2 | Lifecycle rule | **`ensure_mutable()`** — DRAFT and OPEN permitted, LOCKED rejected, re-read inside the transaction |
| D5 | Persist rejected submissions | **NO** — a refusal writes nothing at all |
| D9 | `source_version` | **ADVISORY** — never blocks, no column, last-write-wins |
| D11 | Submission identity | **FROZEN**, and widened by O-1 |
| D12 | Canonical hash | **REUSE** Step 9's `app_remote::content_hash`; no second algorithm |
| D13 | Signing | **NONE** |
| D14 | `Actor::RemoteImport` construction | **`src-tauri` only**, from database-resolved rows |
| O-1 | Cross-participant artefact detection | **APPROVED** — enforced in the domain transaction |
| O-2 | File delivery | **Tauri core drag/drop**, path held in Rust, no plugin |
| O-3 | Host UI placement | **Meeting-level**, dedicated `ImportSubmissionPanel.tsx` |
| O-4 | Error model | **Explicit domain errors** for duplicate and modified artefact |
| O-5 | PRD reconciliation | **Recorded, not edited** — see below |
| O-6 | Import size limit | **256 KiB**, checked before and after the read |
| O-7 | Preview → confirm carrier | **Rust-held pending import state** |

### O-1 — submission identity, widened

Given a submission for the Host-selected meeting:

| Condition | Verdict |
| --- | --- |
| `(meeting, participant, submission_id)` seen, same `content_hash` | **DUPLICATE** — reject |
| `(meeting, participant, submission_id)` seen, different `content_hash` | **MODIFIED_ARTIFACT** — reject |
| `(meeting, submission_id)` seen under a **different** participant | **CROSS_PARTICIPANT_ARTIFACT** — reject |
| different `submission_id` | **NEW** — import |

All three checks run **inside the domain transaction**, not only in the preview
or the UI. A check that lives only in a preview is a race.

The third row extends the earlier D11 interpretation, which keyed detection per
participant: an artefact whose `participant_id` was edited produces a different
canonical hash — `participant_id` is a hashed field — so it would have missed the
per-participant lookup entirely.

**None of these is a correction mechanism.** A correction is a newly generated
remote form, which carries a new `submission_id` and imports as NEW. There is no
Host "replace" action for remote submissions in this step.

`UNIQUE(meeting_id, participant_id, submission_id, content_hash)` remains the
backstop for a race the lookups cannot see; violating it is reported as a
duplicate, not as a failure.

### O-2 — the file arrives by drag and drop

Tauri's core drag-drop delivers the dropped path to **Rust**, which keeps it in a
small Tauri-managed pending-import state. The renderer is told a file is waiting
and calls preview with a meeting id and **no path**. A renderer that cannot name
a path cannot read an arbitrary file — the same principle Step 9 applied to
writing.

The handler delegates to a separately testable function rather than holding logic
inside `run()`, which no integration test can execute.

A paste-JSON field is the fallback, feeding the same pipeline with no filesystem
involved.

**No `tauri-plugin-fs`, no `tauri-plugin-dialog`, no capability change.**

### O-3 — Host UI placement

Import is **meeting-scoped**, so the surface is meeting-level, not a participant
roster row. A dedicated `ImportSubmissionPanel.tsx` is mounted from
`MeetingDetailView.tsx`, which is touched only as far as mounting requires.

No roster redesign. No bulk import. No import history UI.

### O-4 — error model

`DomainError` gains `DuplicateSubmission` and `ModifiedArtifact` (and the
cross-participant case). They are determined **inside the atomic transaction**,
which is why they are domain errors rather than `RemoteError` variants: moving
them out would mean checking outside the transaction, which is a race.

A generic SQLite conflict is **not** the primary API; it remains the backstop.
Transport and UI expose stable error kinds and leak no raw database diagnostics.

### O-5 — PRD reconciliation, recorded not applied

Frozen MVP behaviour: NEW imports; same id + same hash rejects as duplicate; same
id + different hash rejects as modified artefact; same id under another
participant rejects as cross-participant artefact. **No replace, no correction.**

The following existing wording is formally inconsistent with that and is
**flagged for a future documentation reconciliation, not rewritten**:

- `PRD-LAN-Meeting-Collaboration-App.md` line **421** — *"Host tetap mengontrol
  keputusan duplicate (tolak / gantikan / jadikan versi baru) sebelum persist."*
- `PRD-LAN-Meeting-Collaboration-App.md` line **429** — *"Host mendapat
  informasi yang cukup untuk memilih tolak / gantikan / versi baru."*
- `PRD` line **428** — *"mengimpor versi koreksi dari participant yang sama dapat
  dibedakan dari file yang sama persis"* (the distinction still holds; what
  changed is that the correction is refused rather than imported).
- `Architecture-Engineering-Rules-…md` lines **342-344** — the *ditolak /
  menggantikan versi sebelumnya / dibuat sebagai versi baru* choice.
- `Architecture-Engineering-Rules-…md` line **351** — *"`submission_id` sama
  tetapi `content_hash` berbeda → koreksi dari participant yang sama"*.

ADR-0022 records the supersession. Neither document is edited during this freeze.

### O-6 — size limits

| Limit | Where | Value |
| --- | --- | --- |
| complete submission JSON | `src-tauri`, at read time | **256 KiB** |
| note content | `app-core::note`, in the transaction | 64 KiB, unchanged and authoritative |
| `raw_payload` | bounded by the file limit | **no database constraint** |

Checked **before** the file is read and again on the result, so a file that
changes size mid-operation cannot bypass the bound. Nothing reads a file of
unknown size into memory. Filenames are never parsed and never used as identity.

### O-7 — preview and confirmation

**Preview is read-only** and runs entirely on the read-only connection pool,
which is opened `SQLITE_OPEN_READ_ONLY` — a write through it is refused by SQLite
rather than by discipline.

**Confirmation repeats every security-critical check against current state.**
Between the two a meeting can be locked, a participant removed, another
submission imported or the note changed. The preview's eligibility, resolved ids,
hash and duplicate verdict are **advisory display** and none is carried into the
write as authority.

The bounded artefact bytes stay in the Rust-side pending state; the renderer
never sends the artefact back as the authority for confirmation.

### The transaction — six invariants

```text
BEGIN IMMEDIATE
  load meeting
  authorize the RemoteImport actor     -- inherited from the step 8 note write
  ensure_mutable                       -- the lock, re-read here
  validate the note content
  resolve the participant in this meeting
  check submission identity            -- duplicate / modified / cross-participant
  apply the note write                 -- upsert + version, shared with write_note
  insert remote_submissions
  insert audit_logs
COMMIT
  publish note.changed
```

1. the lock is checked inside the transaction;
2. actor authorization is checked inside the transaction;
3. participant confinement is checked inside the transaction;
4. duplicate / artefact identity is checked inside the transaction;
5. note, version, ledger row and audit entry commit atomically;
6. the event is emitted only after the commit.

**Authorization precedes the lock, and that is not a step 10 decision.** It is
the sequence `Domain::write_note` has shipped with since step 8, and the shared
gate below preserves it rather than reopening it: `app-server` maps `Forbidden`
and `MeetingLocked` onto different answers, so swapping the two would change what
an unauthorized LAN participant learns about a locked meeting. This line is
documentation alignment with already-shipped behaviour, not a new runtime
decision. ADR-0022 decision 5 records the same thing.

Nothing import depends on is affected. Lifecycle enforcement still happens
**inside the same transaction**, before any note mutation and before any
submission-identity decision.

**Submission identity is checked only after the security and lifecycle gate.**
A locked meeting must be refused *as locked* even when the file offered is also a
duplicate or a modified artefact, and an actor with no standing must be refused
*as unauthorized* even when the artefact it carries is one this application has
already seen. Artefact identity is never an authorization shortcut and never a
lifecycle one.

**`Domain::write_note` must not be called as a separate transaction.** The
accepted structure is **private in-transaction helpers extracted from
`write_note`**, preserving the existing step 8 sequence exactly, with
`write_note` and `import_remote_submission` as their two callers:

| Helper | Owns |
| --- | --- |
| the gate | meeting load, authorization, the lock, content validation, membership - and returns the authorization proof |
| the write | the upsert, the version derivation, the `note_versions` row |

An import runs its identity lookups *between* the two. The ledger row is
inserted with the same proof, so `proof.meeting_id()` scopes it.

The authorization sequence, note-version derivation and lock semantics must
exist in **one** implementation.

### Frozen behaviour for the awkward cases

| Situation | Behaviour |
| --- | --- |
| participant id exists, name differs | **warning**; import permitted; the database name wins |
| participant id does not exist | **hard error** |
| participant belongs to another meeting | **hard error**; also structurally impossible |
| submission meeting differs from the Host's selection | **hard error**, before confirmation |
| displayed meeting metadata differs from the database | **warning**; nothing from the file is stored |
| `source_version` stale | **not an error** — imports as a new version |
| timestamps malformed or implausible | **not an error** — display only, excluded from the hash |
| participant removed from the roster | **hard error** |
| LAN session revoked, still on the roster | **imports normally** — revocation governs LAN access, and remote participation is session-less |

### Ledger, audit and events

`resolution` is `IMPORTED` when the note was created and `REPLACED` when its
content was replaced. No third value.

`raw_payload` stores the **exact submitted JSON, verbatim** — not a
re-serialisation. Canonical bytes exist only to be hashed. The HTML artefact is
never stored.

One audit row per successful import: `remote_submission.imported`, target
`Note`, with `participant_id`, `submission_id`, `content_hash`, `source_version`,
`note_version` and `resolution` in metadata. `actor_type`, `actor_id` and
`meeting_id` come from the proof automatically. No new audit table, no new
`AuditTarget` variant, append-only behaviour untouched.

Events reuse `DomainEvent::NoteChanged` unchanged — identifiers and a version,
never the Markdown — published after the commit. **No new event variant.** A
failed import emits nothing.

### No migration

`V1` and `V2` remain the only migrations. Verified column by column: the ledger
row, its idempotency key, the hash format, the resolution/version coherence
check, import attribution, the audit action and metadata, the raw payload and the
cross-meeting foreign key are all already expressible.

### Expected implementation file plan

Change only what the implementation actually requires, from this list:

```text
crates/app-core/src/service.rs          extract the helper; add import_remote_submission
crates/app-core/src/port.rs             NewRemoteSubmission; find/insert on DomainTx
crates/app-core/src/audit.rs            AuditAction::RemoteSubmissionImported
crates/app-core/src/error.rs            DuplicateSubmission, ModifiedArtifact, cross-participant
crates/app-core/src/lib.rs              re-exports
crates/app-db/src/repository.rs         implement the new DomainTx methods
crates/app-db/src/query.rs              preview-side ledger lookup
crates/app-remote/src/lib.rs            a size-limit error
src-tauri/src/host.rs                   preview + import; the only Actor::RemoteImport
src-tauri/src/lib.rs                    drag-drop handler, pending state, command registration
src-tauri/src/commands.rs
src-tauri/src/dto.rs
src-tauri/src/error.rs
src-tauri/tests/boundaries.rs
src-tauri/tests/remote_import_commands.rs
crates/app-db/tests/remote_import.rs
apps/host-ui/src/api/hostApi.ts
apps/host-ui/src/components/ImportSubmissionPanel.tsx
apps/host-ui/src/components/MeetingDetailView.tsx
apps/host-ui/src/styles.css
packages/contracts/src/host.ts
packages/contracts/src/index.ts
docs/adr/0022-remote-submission-import.md
```

### Expected untouched areas

```text
crates/app-db/migrations/**
crates/app-core/src/authz.rs
crates/app-core/src/actor.rs
crates/app-core/src/note.rs
crates/app-server/**
apps/lan-ui/**
apps/remote-form/**
packages/editor/**
crates/app-export/**
scripts/check-remote-form-offline.mjs
src-tauri/capabilities/default.json
docs/adr/0001 - 0021
```

`authz.rs` and `actor.rs` in particular need **no change**: the authorization
rule for import has existed and been tested since step 2.

If implementation reveals that one of these must change, **stop and report it as
an unresolved design issue** rather than silently expanding scope.

### Approved test scope

**`app-remote`.** Parse valid v1; malformed JSON; wrong schema version; invalid
ids; the new size-limit error; canonical hash compatibility with the shared
fixtures.

**`app-db` / domain.** First import (version 1, `IMPORTED`); import over an
existing note (N+1, `REPLACED`); attribution is `REMOTE_IMPORT` with the
participant id; duplicate refused with exactly one ledger row and no second
version or audit row; modified artefact refused with nothing written;
cross-participant artefact refused; two distinct submission ids both succeed with
last-write-wins; wrong meeting; unknown participant; removed participant;
**revoked session but still on the roster succeeds**; locked meeting refused with
zero rows in all four tables; stale `source_version` succeeds; forced failure at
each stage rolls back everything; the `UNIQUE` race is a duplicate not a failure;
audit row content; ledger row content; `note.changed` carries a version and no
content; no event on failure.

**Tauri.** Preview mutates nothing (row counts before and after); preview of a
duplicate reports it without writing; confirmation re-checks after the meeting is
locked post-preview; confirmation re-checks after the participant is removed
post-preview; the Host-selected meeting cannot be bypassed; the actor is built
only from resolved ids; oversized file refused before the read; non-UTF-8
refused; missing file refused; the returned version matches the database.

**Host UI.** Preview; name-mismatch warning; duplicate; modified artefact;
explicit confirmation; result.

**Guards.** No LAN HTTP import route; no browser-side import authority;
`Actor::RemoteImport` only in `src-tauri/src/host.rs`; no SQL in `app-remote`; no
rejected-submission write; no credential or signing mechanism; no capability
expansion; no migration; no Step 11 lock implementation; `note_links` still
schema-only.

### Explicitly not part of Step 10

No new LAN API; no changes to participant LAN editing; no structured note links;
no merge; no optimistic concurrency; no form invalidation or expiry; no signing
or HMAC; no cloud; no automatic upload; no bulk import; no import history UI; no
PDF; no AI; no calendar; no lock implementation beyond consuming the existing
`ensure_mutable()` rule; and no changes to remote-form generation unless a
compatibility defect is proven.

---

## Step 11 — Meeting Lock

**Status: COMPLETE** (uncommitted)

From `README.md` roadmap item 11 and PRD section 19.

A read-only design inspection preceded implementation and was frozen in
`docs/implementation-prompts/step-11-meeting-lock.md` before any code changed.

Scope: exposed the already-implemented `Domain::lock_meeting` through the Host
Tauri command surface (`host.rs` → `commands.rs` → `lib.rs`, moving the
registered-command / gateway parity guard from 28 to 29) and through the Host
UI (`hostApi.lockMeeting`, and a two-step in-component confirmation on
`MeetingDetailView` shown only while a meeting is `OPEN`). No domain, schema,
contract, DTO, error, event or LAN protocol change was needed or made - the
lock rule, its transaction, its audit record and its event already existed and
already worked; this step only made the Host able to reach them.

Test coverage added: command-layer coverage for a successful lock, the
`DRAFT -> LOCKED` refusal, the `LOCKED -> LOCKED` refusal, an unknown meeting,
a malformed identifier, and the exactly-one `meeting.locked` audit entry
(`src-tauri/tests/host_commands.rs`); explicit authorization coverage proving
`Actor::RemoteImport` cannot perform `Operation::LockMeeting`
(`crates/app-core/src/authz.rs`). The existing locked-mutation command-layer
test now reaches `LOCKED` through the new command instead of calling
`Domain::lock_meeting` directly.

---

## Step 12 — Export: Markdown and TXT / AI Context

**Status: COMPLETE** (uncommitted)

From `README.md` roadmap item 12, PRD sections 6, 16, 20, 21 and 25.4,
ADR-0004, ADR-0023 and architecture rules sections 17, 19, 19.1 and 21.

A read-only design inspection, an explicit resolution of eight material
ambiguities, and four corrections to that resolution all preceded
implementation and are recorded in ADR-0023 before any code changed.

Scope delivered: `crates/app-export` is a pure rendering layer (no
filesystem, database, Tauri or authorization dependency) producing
deterministic Markdown, TXT and AI Context documents from a plain
`ExportDocument`. This is where the Rust Markdown renderer lands - a bounded
parser (`markdown_ast`) mirroring `packages/editor/src/parse.ts`'s subset,
feeding a plain-text tree-walk (`plain_text`) held to the same
`packages/editor/__fixtures__/notes.json` fixture oracle the TypeScript side
already uses, satisfying ADR-0007's requirement that the two renderers agree
on one subset.

Export is permitted from `OPEN` and `LOCKED`, refused from `DRAFT`, enforced
twice - a cheap read-only gate before rendering (`HostState::
check_export_eligible`) and the actual transactional enforcement inside a
new, audit-only `Domain::record_export` (new `Operation::GenerateExport`,
new `AuditAction::MeetingExported` -> `"meeting.exported"`). The filesystem
write and the audit write are not atomic, and no distributed-transaction
mechanism was introduced to fake it (ADR-0023 decision 2). AI Context
performs no heading recognition or semantic classification of any kind - the
PRD's "distinguish discussion/decision/action item" is satisfied as a side
effect of faithful structural preservation, never by inspecting a heading's
text. Exported files never contain `generated_at`, a participant id, a claim
status or any session material.

No migration, no new Tauri capability, no CSP change, no new LAN route or
WebSocket message, and no third-party Markdown/PDF dependency. **PDF remains
Phase 2 by decision (ADR-0004), unchanged.**

---

## Current Execution Point

> Phase 1 is complete and pushed through Step 10. **Step 11 — Meeting Lock —
> is committed locally, not yet pushed. Step 12 — Export — is implemented
> and awaiting review in the working tree.** Phase 1's MVP scope (PRD section
> 24) is now functionally complete; only documentation review, validation and
> the commit/push of Steps 11 and 12 remain.

Repository state:

- branch `main`, `HEAD` = `a9ef7f8bbd51509cef70928092f5a74df71956f8`
  (`feat: expose meeting lock to host`)
- Steps 0 through 11 committed; Step 11 not yet pushed to `origin/main`;
  Step 12 changes are uncommitted
- Rust and TypeScript suites both passing
- no migration exists beyond `V1` and `V2`; neither Step 10, 11 nor 12 added
  one

The remote participation loop is closed end to end: a Host generates an
offline form, a participant fills it in and exports a submission, and the Host
previews and imports it back into that participant's single note. The Host
can lock a meeting, which is irreversible and refuses every mutation already
covered by the domain's `ensure_mutable()` gates, and can now export the
meeting record as Markdown, TXT or AI Context from `OPEN` or `LOCKED`. What
remains in Phase 1 is committing and pushing Steps 11 and 12; PDF export
stays deferred to Phase 2 (ADR-0004).
