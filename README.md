# LAN Meeting Collaboration App

Local-first desktop app for running structured meetings on a local network.

The Host runs the application on one computer. Participants on the same network
join from a browser - no install, no account. Participants who cannot reach the
network receive a single self-contained HTML file, fill it in offline, and send
a submission file back for the Host to import.

Everything is stored in a local SQLite database on the Host device.
**No cloud, no external API, no public hosting, no AI API.**

> **Status: the meeting works end to end.** A Host can create a meeting,
> configure it, build a roster of up to 99 participants, open it, start the LAN
> server and show a join link and QR code. Participants claim an identity from a
> browser on the same network and write their own note in a shared Markdown
> editor; the Host sees every note, can edit any of them, and can read the full
> version history. Both sides update themselves in realtime. For someone who
> cannot reach the network, the Host generates a single offline HTML form that
> the participant fills in and exports as a submission file, and the Host
> reads it back into that participant's note. The Host can lock a meeting,
> ending editing for everyone with no way back, and can export the meeting
> record as Markdown, TXT or AI Context. Note links are not implemented yet;
> PDF export is deferred to Phase 2. See [Roadmap](#roadmap).

## How it works

```text
                Host device (Tauri app)
   +-----------------------------------------------+
   |  Host UI (React)  ->  Rust core  ->  SQLite    |
   |                         |                      |
   |                   LAN HTTP + WebSocket         |
   +-------------------------|---------------------+
                             |  local network
                      LAN participants (browser)

   Remote participant:  HTML file  ->  browser (offline)
                        ->  submission file  ->  Host imports
```

- SQLite is the source of truth. The WebSocket only notifies: an event names
  what changed and the client re-reads it over HTTP, so a missed event costs a
  refetch and nothing else ([ADR-0018](docs/adr/0018-realtime-events-and-presence.md)).
- Authorization, meeting lock and auditing are enforced in Rust, never in the UI.
- A submission file is untrusted input, even though this app generated the form
  it came from.

## Documentation

| Document | Purpose |
| --- | --- |
| [PRD](PRD-LAN-Meeting-Collaboration-App.md) | Product requirements (authoritative) |
| [Architecture & Engineering Rules](Architecture-Engineering-Rules-LAN-Meeting-Collaboration-App.md) | Architecture and engineering rules (authoritative) |
| [docs/adr](docs/adr/) | Decisions and their rationale |
| [CLAUDE.md](CLAUDE.md) | Engineering checklist for contributors and AI assistants |

## Repository layout

```text
apps/
  host-ui/        React UI inside the Tauri WebView (Host)
  lan-ui/         React UI served to LAN participants, embedded in the binary
  remote-form/    Offline single-file HTML form for remote participants
packages/
  contracts/      Shared TypeScript boundary shapes (API, events, submission)
  editor/         Shared note editor, Markdown subset, renderer and validator
crates/
  app-core/       Domain core: actors, authorization, lock, audit, versioning
  app-db/         SQLite: connections, migrations, repositories
  app-server/     LAN HTTP + WebSocket transport
  app-remote/     Remote form generation and submission import pipeline
  app-export/     Deterministic Markdown and TXT / AI Context renderers
src-tauri/        Tauri shell: app state, Host commands, server lifecycle
scripts/          Build guards (offline self-containment of the remote form)
docs/adr/         Architecture decision records
```

The three UI bundles are separate deliberately: the Host UI is never served over
the network, and the remote form never talks to the Host.

## Prerequisites

| Requirement | Notes |
| --- | --- |
| [Rust](https://rustup.rs) stable, MSVC toolchain | `rustup default stable-x86_64-pc-windows-msvc` |
| Visual Studio 2022 Build Tools | Workload: *Desktop development with C++* (MSVC v143 + Windows SDK) |
| [Node.js](https://nodejs.org) 20.19+ or 22+ | npm workspaces |
| WebView2 Runtime | Preinstalled on current Windows 11 |
| Git | |

On Linux or macOS, follow the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for that
platform instead of the two Windows-specific rows.

**Firewall:** the LAN server binds a LAN-reachable address. On first run Windows
will ask to allow it. Participants cannot connect if the network profile is
*Public* or the inbound rule is denied.

## Getting started

```bash
npm install          # installs all workspaces
npm run dev          # tauri dev: builds host-ui and opens the desktop app
```

Useful commands:

```bash
npm run dev:host           # host UI alone, in a browser
npm run dev:lan            # LAN participant UI alone
npm run dev:remote         # remote form template alone
npm run build:web          # build all three UI bundles
npm run build:lan          # participant UI; app-server embeds this bundle
npm run build:remote       # build the form + verify it is offline-self-contained
npm run typecheck          # TypeScript across all workspaces
npm run test:ts            # Vitest: the shared editor package
npm run check              # typecheck + vitest + rustfmt + clippy + cargo test
npm run tauri build        # package the desktop app
```

## Roadmap

Phase 1 is built in order, each step independently demonstrable:

1. Environment and skeleton
2. Data spine: migrations, repositories, audit triggers
3. Domain core: actors, authorization, lock-in-transaction, versioning
4. Meeting and participant management in the domain
5. Host UI: meeting lifecycle, participants, audit view
6. LAN server, join flow, identity claim, QR
7. Realtime: audience-scoped WebSocket and presence
8. Host note editing and version history
8B. Participant note editing over the LAN
9. Remote form generation
10. Remote submission import pipeline
11. Meeting lock
12. Export: Markdown and TXT / AI Context *(current)*

PDF export is Phase 2 by decision - see
[ADR-0004](docs/adr/0004-defer-pdf-export.md).

## Privacy and security posture

- Meeting data never leaves the Host device except over the local network, or as
  files the user chooses to send.
- The LAN server is intended for local networks only and must never be exposed
  to the internet. No database port is opened.
- LAN transport is plain HTTP; the join URL and QR code are operational secrets.
  This is a stated boundary, documented in
  [ADR-0002](docs/adr/0002-lan-identity-first-claim-wins.md).
- The notification socket carries the session token in the WebSocket
  subprotocol, never in a URL, and carries identifiers rather than content. A
  participant never receives another participant's event; the audience is
  decided in Rust, per event
  ([ADR-0018](docs/adr/0018-realtime-events-and-presence.md)).
- A participant can read and write only their own note. The note routes accept
  no participant or meeting identifier at all: identity comes from the
  authenticated session, so addressing somebody else's note is inexpressible
  rather than merely refused
  ([ADR-0020](docs/adr/0020-participant-note-editing.md)).
- Note content is GFM-subset Markdown and is treated as untrusted input
  everywhere it is displayed, including inside the Host application. It is
  rendered as DOM nodes by `packages/editor`, never as an HTML string, and link
  schemes are limited to `http`, `https` and `mailto`
  ([ADR-0019](docs/adr/0019-note-editing-and-markdown-subset.md)).
- No telemetry, no analytics, no crash reporting, no CDN assets.

## License

MIT
