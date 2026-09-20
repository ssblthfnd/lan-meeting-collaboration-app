# 0009. Remote form uses no UI framework

- Status: Accepted
- Date: 2026-09-20

Amends ADR-0006 for `apps/remote-form` only. ADR-0006 otherwise stands.

## Context

ADR-0006 gave all three UI bundles the same starting dependency set, including
`react`, `react-dom` and `@vitejs/plugin-react`. That was the cheap default at
skeleton time, before anything enforced the remote form's offline contract.

The first run of `scripts/check-remote-form-offline.mjs` against a built
artefact rejected it, for two reasons that both came from the build tooling
rather than from our own code:

1. Vite's module-preload polyfill emits a `fetch()` call to warm
   `modulepreload` links. In a single-file build there is no module graph to
   preload, so the call is dead code - but it is a real `fetch(` in the
   artefact.
2. React's production error helper builds an absolute `https://` documentation
   URL from an error code. It is a diagnostic string that is never requested
   over the network, but it is an absolute external URL in a file whose stated
   contract is that it contains none.

Neither was a live network call. Both were real violations of the property the
guard exists to protect, and the guard is deliberately literal: it reads the
built bytes, because that is all a participant's offline machine will have.

Three routes were considered. Relaxing the guard's exclusions was rejected
outright - the engineering rules state the check must not be weakened, and an
exclusion list is exactly how this property erodes. Stripping React's error URL
with a build-time string replacement would have kept React, at the cost of a
rollup plugin whose only job is to launder a dependency's output, plus degraded
React error messages. That leaves removing the framework.

The deciding factor is that the remote form is not a framework-shaped problem.
It renders a fixed set of fields, validates them locally, and serialises a
submission on an explicit user action. There is no server state, no routing, no
concurrent rendering, and no shared session. React's value here was developer
familiarity, and it was charging a 225 kB artefact and two guard violations
for it.

## Decision

`apps/remote-form` is built with plain TypeScript and DOM APIs. No UI
framework, and no replacement framework.

- `react`, `react-dom`, `@types/react`, `@types/react-dom` and
  `@vitejs/plugin-react` are removed from that workspace only.
- `apps/host-ui` and `apps/lan-ui` keep React. They run in a live, stateful
  environment and are under no offline-artefact constraint.
- `vite.config.ts` sets `build.modulePreload: false`, which removes the preload
  polyfill's `fetch()`. Nothing in a single-file bundle needs preloading.
- `scripts/check-remote-form-offline.mjs` is unchanged. Its exclusion list
  still covers only the W3C namespace URIs that XML and SVG require.
- Rendering uses `document.createElement` and `textContent`. This bundle
  assigns `innerHTML` nowhere: meeting and participant content is hostile input
  here too, and an HTML sink inside a file that runs from `file://` is a
  sanitiser bypass no backend check can reach.

`packages/editor` is shared by all three bundles and is therefore also bound by
this decision: whatever it exports must be usable without a framework runtime.
Its implementation arrives in Phase 1 steps 6 and 7.

## Consequences

- The built artefact is 1.39 kB, down from 225.22 kB. For a file that travels
  by email or USB stick to a participant with no internet, that is the point.
- The offline guard passes on its own terms, with no exclusion added for a
  dependency's convenience.
- The remote form and the other two bundles now render differently. The shared
  contract stays the data in `packages/contracts` and the note format in
  `packages/editor`, not the rendering technique.
- `packages/editor` cannot be a React component library. It must expose
  framework-neutral logic - parse, sanitise, serialise - that each bundle
  wraps. This is a constraint discovered here and paid for later, in steps 6
  and 7.
- Form state is managed by hand. At the size the PRD describes - one note, a
  bounded set of fields, at most five links - that is a small amount of
  explicit code. If the form grows past what plain DOM handling can carry
  readably, that is a superseding ADR, not a quiet `npm install`.
