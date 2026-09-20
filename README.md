# LAN Meeting Collaboration App

Local-first desktop app for running structured meetings on a local network.

The Host runs the application on one computer. Participants on the same network
join from a browser - no install, no account. Participants who cannot reach the
network receive a single self-contained HTML file, fill it in offline, and send
a submission file back for the Host to import.

Everything is stored in a local SQLite database on the Host device.
**No cloud, no external API, no public hosting, no AI API.**

> **Status: project skeleton.** No features are implemented yet. See
> [Roadmap](#roadmap).

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

- SQLite is the source of truth. WebSocket only notifies; it never carries
  authority.
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
  editor/         Shared note editor + Markdown renderer (lands in step 7)
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
npm run build:remote       # build the form + verify it is offline-self-contained
npm run typecheck          # TypeScript across all workspaces
npm run check              # typecheck + rustfmt + clippy + cargo test
npm run tauri build        # package the desktop app
```

## Roadmap

Phase 1 is built in order, each step independently demonstrable:

1. Environment and skeleton *(current)*
2. Data spine: migrations, repositories, audit triggers
3. Domain core: actors, authorization, lock-in-transaction, versioning
4. Host UI: meeting lifecycle, participants, audit view
5. LAN server, join flow, identity claim, QR
6. Realtime: audience-scoped WebSocket
7. Host note editing and version history
8. Remote form generation
9. Remote submission import pipeline
10. Meeting lock
11. Export: Markdown and TXT / AI Context

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
- No telemetry, no analytics, no crash reporting, no CDN assets.

## License

MIT
