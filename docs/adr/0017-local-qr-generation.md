# 0017. Local QR generation

- Status: Accepted
- Date: 2026-09-21

Adds one crate to the dependency set ADR-0006 committed to. That table was
described there as "a commitment, not a shortlist", so an addition it does not
contain needs recording.

## Context

PRD section 8.1 requires the Host to display a QR code for the join URL, and
section 23 requires it to be generated locally. A participant scanning a code
off a projector is the primary way people join, so this is not decoration.

ADR-0006 enumerated the intended dependencies for every Phase 1 step and listed
no QR crate. Adding one silently would make that table untrue, and the table's
value is that it can be read as complete.

Three options:

1. **A Rust encoder in `src-tauri`.** One small crate; the URL never leaves Rust.
2. **A JavaScript encoder in the Host UI.** An npm dependency where a Rust one
   fits, and it moves URL handling into the frontend.
3. **Hand-writing an encoder.** QR is a specified format with Reed-Solomon error
   correction, masking and version selection. Writing it would be more code than
   the rest of this step, and a subtly wrong code fails by not scanning.

## Decision

Option 1: **`qrcode`**, with `default-features = false`.

Turning default features off drops its `image` dependency, which drags in
encoders, decoders and colour handling for a job that needs none of them. What
remains is the encoder.

### The output is a matrix, not an image

The crate can render SVG and PNG. Neither is used. The Tauri command returns:

```rust
struct QrMatrix { size: u32, modules: Vec<bool> }
```

and the Host UI draws `<rect>` elements from it. Three reasons, in order:

1. **No markup injection.** Returning an SVG *string* would mean the UI injecting
   it with `dangerouslySetInnerHTML`. That is a habit worth not starting in an
   application whose premise is that content is untrusted (architecture rules
   13.1), even when this particular string is our own.
2. **No new CSP allowance.** A `data:` image URI would work under the existing
   policy, but a grid of rectangles needs nothing at all, and the Tauri CSP stays
   exactly as it was.
3. **The smallest surface.** Only the encoder is compiled; no image format, no
   file writing, no rasteriser.

Error correction level **M** (~15% recoverable). A join URL is short, so the
denser grid costs little, and the redundancy is what keeps the code scannable
from a projector or at an angle.

### One assembly point for the URL

The join URL is built once, in `app_server::network::join_url`, and both the
displayed text and the QR are made from that string. The Host UI cannot show one
URL and encode a different one.

## Consequences

- `qrcode` joins the dependency set. It is pure Rust, has no network or
  filesystem access, and is used in one module.
- The QR renders in whatever size the UI asks for without re-encoding, and stays
  sharp because it is vector rectangles rather than a scaled bitmap.
- The rendered code keeps a white background regardless of the colour scheme. A
  dark-on-dark QR does not scan, so this one element deliberately ignores the
  app's theme.
- Tests assert the grid is square, that the three finder patterns are where a
  scanner looks for them, that encoding is deterministic, and that a longer URL
  produces a larger grid - enough to catch a placeholder or a wired-up-wrong
  encoder without decoding the image.
- If a future step needs a QR as a file - a printable sheet, say - that is a
  rendering decision to make then, and it does not change this one.
