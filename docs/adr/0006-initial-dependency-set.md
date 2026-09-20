# 0006. Minimal initial dependency set

- Status: Accepted
- Date: 2026-09-20

## Context

The architecture analysis produced a fairly long candidate dependency list.
Installing all of it at skeleton time would lock in choices before the code that
needs them exists, and would make the first build slow and hard to audit for an
open-source repository that promises no cloud, no telemetry and no CDN.

## Decision

The skeleton carries the smallest set that lets the workspace build.

**Rust, added now**

| Crate | Where | Why now |
| --- | --- | --- |
| `tauri`, `tauri-build` | `src-tauri` | The desktop shell must compile. |
| `serde`, `serde_json` | core, remote, tauri | Boundary types exist from the start. |
| `thiserror` | all crates | Every crate defines a typed, actionable error enum. |

**TypeScript, added now**

`react`, `react-dom`, `typescript`, `vite`, `@vitejs/plugin-react`,
`@tauri-apps/api` (host-ui only), `vite-plugin-singlefile` (remote-form only),
and the matching `@types` packages.

**Selected but deliberately not yet added**

These are the intended choices; each arrives with the step that needs it, so
that its first commit also contains its first use and its tests.

| Crate | Purpose | Arrives at |
| --- | --- | --- |
| `rusqlite` (feature `bundled`) | SQLite driver. `bundled` compiles SQLite in, so there is no system dependency on Windows. | Step 1 |
| `r2d2` + `r2d2_sqlite` | Read pool alongside a single writer connection. | Step 1 |
| `refinery` | Embedded migrations, offline. | Step 1 |
| `jiff` or `chrono` + `chrono-tz` | UTC storage and IANA timezone conversion (ADR-0005). | Step 1 |
| `uuid` (v7) | Time-ordered identifiers. | Step 1 |
| `tokio` | Async runtime for the LAN server. | Step 4 |
| `axum`, `tower-http` | LAN HTTP and WebSocket. | Step 4 |
| `rand`, `sha2`, `subtle` | Join/session tokens: OS entropy, hashing, constant-time compare. Tokens are 256-bit random values, so a password hash is not the right tool. | Step 4 |
| `rust-embed` | Embeds the `lan-ui` bundle and the remote-form template in the binary. | Step 4 |
| `local-ip-address` | Enumerating LAN interfaces so the Host can choose one. | Step 4 |
| `tracing`, `tracing-subscriber` | Local logging. No telemetry, no remote sink. | Step 4 |
| `pulldown-cmark`, `ammonia` | Markdown rendering and sanitisation of participant content. | Steps 6, 10 |
| `insta` | Snapshot tests that guard export determinism. | Step 10 |

`sqlx` was considered instead of `rusqlite`: it offers async and compile-time
checked queries, at the cost of a build-time database and heavier compiles. The
workload here is local, small and write-serialised, so `rusqlite` with `bundled`
was preferred for build predictability on Windows.

**Never**

Cloud services, external APIs, telemetry or crash reporting, authentication
providers, relay servers, CDN-hosted assets or fonts, and any PDF dependency
during Phase 1 (ADR-0004).

## Consequences

- The first build is small and every dependency is explainable.
- Each step's commit introduces its dependency together with its usage.
- The "selected but not yet added" table is a commitment, not a shortlist: a
  different choice at implementation time needs a superseding ADR.
