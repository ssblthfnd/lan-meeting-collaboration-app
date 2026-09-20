# LAN Meeting Collaboration App

## Project Overview

Desktop application for running structured meetings on a local network.

The **Host** runs the app on one computer. Participants take part in one of two
ways:

- **LAN participants** join from a browser on the same network.
- **Remote participants** receive a self-contained HTML form, fill it in
  offline, export a submission file, and send it back for the Host to import.

All data lives in a local SQLite database on the Host device. No cloud, no
external API, no public hosting, no AI API.

## Authoritative References

Read these before implementing anything. They win over this file on product and
architecture questions:

- `PRD-LAN-Meeting-Collaboration-App.md`
- `Architecture-Engineering-Rules-LAN-Meeting-Collaboration-App.md`
- `docs/adr/` - accepted decisions, including the four that shaped the skeleton

This file is a concise engineering checklist, not a summary of those documents.

## Core Architecture

- Desktop shell: Tauri 2
- Frontend: React + TypeScript (Vite)
- Backend: Rust (Cargo workspace)
- Database: SQLite - the source of truth
- LAN: local HTTP server + WebSocket, started by the Host
- Remote participation: self-contained offline HTML form
- Remote submission: structured, versioned submission file

## Three UI Bundles

They are separate on purpose. Do not merge them.

| Bundle | Runs in | Delivered as | Has access to |
| --- | --- | --- | --- |
| `apps/host-ui` | Tauri WebView | Tauri asset bundle | Tauri commands (Host authority) |
| `apps/lan-ui` | Participant browser | static files embedded in the Rust binary | LAN HTTP + WebSocket only |
| `apps/remote-form` | Participant browser, `file://` | one inlined HTML file | nothing - fully offline |

`apps/host-ui` is never served over the LAN. `apps/remote-form` never talks to
the Host.

Shared TypeScript lives in `packages/contracts` (boundary shapes) and
`packages/editor` (the note editor shared by all three bundles).

## Rust Crate Responsibilities

| Crate | Responsibility |
| --- | --- |
| `crates/app-core` | Domain core: actors, authorization, meeting lock, audit policy, note versioning. The only place business rules live. |
| `crates/app-db` | SQLite: connections, PRAGMAs, migrations, repositories, transactions. |
| `crates/app-server` | LAN HTTP + WebSocket transport. Untrusted-input boundary. No business rules. |
| `crates/app-remote` | Remote form generation, submission parsing, validation and import pipeline. |
| `crates/app-export` | Deterministic Markdown and TXT / AI Context renderers. |
| `src-tauri` | Tauri shell: app state, Host commands, LAN server lifecycle. No business rules. |

Both transports (`app-server` and `src-tauri`) must route every mutation through
`app-core`.

## Engineering Rules

### Backend is authoritative

- Authorization, validation and business rules are decided in Rust, never in
  React. UI hiding is not a security mechanism.
- The actor is established at the transport boundary and never read from a
  request body. A `participant_id` sent by a browser is never authority.
- A participant identity binds to one active session (first-claim-wins). The
  Host can approve, reject or revoke a claim. Reconnecting must never change
  identity. Only a hash of the session token is stored.

### SQLite is the source of truth

- No important state may live only in a frontend.
- The database file is local to the Host and is never exposed to the network.
- Exactly one note per participant per meeting:
  `UNIQUE(meeting_id, participant_id)`. Remote import updates that note and adds
  a version; it never creates a second note.
- Note content is **GFM-subset Markdown stored as text**, with an explicit
  allowlist. Raw HTML is rejected; link schemes are limited to `http`, `https`
  and `mailto`. The editor and renderer are shared via `packages/editor`.
- Derived values are not stored twice. Participant claim status is computed from
  `participant_sessions`, never kept as its own column.
- `remote_submissions` records every processed submission
  (`submission_id` + `content_hash`) so that re-importing the same file is a
  detected duplicate rather than a second write.

### WebSocket is not the source of truth

- Order is: HTTP/command mutation, then SQLite transaction, then broadcast.
  Never the reverse.
- Events carry identifiers and versions, not authoritative content. Clients
  re-fetch.
- Channels are audience-scoped: a participant must never receive another
  participant's note.

### Meeting lock is backend-enforced

- Every mutation re-reads meeting status **inside** the mutating transaction.
  A pre-flight check is a race, not an enforcement.
- A disabled button is not enforcement.

### Remote submission is untrusted input

- The app generated the form, but the file that comes back is external data.
- Never trust the filename, or a name or id in the payload, as identity.
- Validate schema, meeting, participant, integrity, duplicates and link count
  before any write. Valid JSON does not mean valid data.
- Import runs in a single transaction. Partial import is not allowed.

### Remote form has zero external dependencies

- One HTML file that opens offline from `file://`.
- No CDN, no external stylesheet or font, no `fetch`, no WebSocket, no upload.
- A submission leaves the browser only when the participant asks for it.
- `npm run build:remote` runs `scripts/check-remote-form-offline.mjs`, which
  fails the build if this is violated. Do not weaken that check.

### Audit and versioning

- Important mutations are audited: actor, action, target, metadata, timestamp.
- The audit log is append-only and is not editable through any application
  flow.
- Note changes produce `note_versions` rows whose `created_by_type` records
  whether the actor was `HOST`, `PARTICIPANT` or `REMOTE_IMPORT`. History is
  never deleted.

### Time and ordering

- System timestamps are stored in UTC (RFC 3339 / ISO 8601).
- A meeting stores its `timezone` explicitly as an IANA identifier. Never rely
  implicitly on the OS timezone for meeting meaning.
- UI and exports present meeting times in the meeting's timezone and state that
  timezone explicitly.
- Every query feeding UI or export has an explicit total ordering. Never rely on
  insertion order.

### Security boundaries

- The LAN server is for the local network only. Never assume `127.0.0.1` is
  reachable by participants, and never expose the app to the public internet.
- No database port is opened to the LAN.
- LAN transport is plain HTTP: the join URL/QR is an operational secret. This is
  a stated, accepted boundary, not an oversight.
- Participant content is hostile input in the Host UI too - sanitize before
  rendering, and allowlist link schemes.
- No cloud services, external APIs, telemetry, authentication providers, relay
  servers, or CDN resources. Ever.

### Errors

- Errors must be actionable and name the expected and the detected value.
  "Something went wrong" is not acceptable when better information exists.

### Scope discipline

- Do not implement features outside the PRD without explicit approval.
- Do not change the architecture without explicit approval; explain the
  implications first.
- Prefer small, focused changes. Do not modify unrelated files.
- Run the relevant checks after implementing: `npm run check`.
- Every new feature must answer the questions in architecture rules section 25.

## Git Commit Rules

These are mandatory.

- Commits must use the repository owner's Git identity:
  `Luthfianda Salsabila <luthfiandasalsabila@gmail.com>`.
- Never use Claude, Anthropic, or any AI identity as the commit author.
- Never add `Co-Authored-By` trailers for Claude or any AI assistant.
- Never add Claude/Anthropic attribution to commit messages or pull request
  descriptions.
- Never modify Git `user.name` or `user.email`. The repository owner configures
  these manually.
- Preserve the existing Git author identity before creating a commit.
- Never create a commit using an AI-generated author identity.

## Repository

<https://github.com/ssblthfnd/lan-meeting-collaboration-app>

Use the existing repository; do not create another one. It is intended to be
public, so never commit secrets, credentials, generated databases, private
meeting data, or generated submission files.


## Development Report Rules

At the end of every implementation or development task, provide a concise report containing:

1. What was completed.
2. Files created or modified.
3. Validation/tests performed and their results.
4. Known issues, blockers, or remaining concerns.
5. **Next Phase / Next Step** — explicitly state the next planned development phase or step according to the roadmap.

The `Next Phase / Next Step` section must always be included, even when the current task is incomplete or blocked.
Do not automatically proceed to the next phase unless explicitly instructed.
