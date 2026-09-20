# @lan-meeting/editor

Shared note editor, allowlist, renderer and sanitiser used by `host-ui`,
`lan-ui` and `remote-form`.

Canonical stored note format is **GFM-subset Markdown as text**
(`docs/adr/0007-note-canonical-format.md`).

Rules for anything added here:

- bundled into the **offline** remote form, so no external CDN resource and no
  network calls
- raw HTML inside Markdown is rejected
- link schemes limited to `http`, `https`, `mailto`
- must agree with the Rust renderer in `crates/app-export` on the same subset,
  guarded by shared fixtures

Implementation arrives with Phase 1 steps 6 and 7.
