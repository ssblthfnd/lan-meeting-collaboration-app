# 0004. Defer PDF export to Phase 2

- Status: Accepted
- Date: 2026-09-20

## Context

The PRD originally listed PDF alongside Markdown and TXT / AI Context as MVP
export formats. PDF is by far the most expensive of the three:

- Pure-Rust PDF crates (`printpdf`, `genpdf`) require manual layout, embedded
  fonts and hand-built table pagination. Notes contain tables and lists, which
  is precisely the hard case.
- Rendering HTML and printing through the WebView shifts the problem to
  platform print behaviour and makes deterministic output harder to guarantee.
- Embedding a layout engine such as Typst is plausible but is a significant
  dependency that needs its own evaluation.

Meanwhile Markdown and TXT / AI Context satisfy most of what section 20 is
actually for: a readable record and a faithful input for an external AI tool.

## Decision

PDF export is deferred to Phase 2.

- Phase 1 export scope: **Markdown** and **TXT / AI Context**.
- No PDF dependency may be added during Phase 1, including "just to try it".
- The PDF approach will be chosen by a separate technical spike, which must
  cover: table and list rendering, page breaks, font embedding for Indonesian
  text, and whether output is byte-for-byte deterministic.
- The spike concludes with its own ADR superseding nothing - this ADR simply
  stops applying once PDF lands.

## Consequences

- Phase 1 ships sooner, and the two formats that feed the AI Context workflow
  are done properly.
- Users who need a PDF in Phase 1 can print the Markdown or convert it with an
  external tool. Accepted.
- `crates/app-export` is designed around a document model that renderers consume,
  so adding a third renderer later does not require restructuring.
