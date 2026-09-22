# 0019. Note editing, the Markdown subset and safe rendering

- Status: Accepted
- Date: 2026-09-22

Implements ADR-0007 (GFM-subset Markdown as the canonical format) and settles
the questions it left open. Depends on ADR-0003 for note cardinality, ADR-0009
for the framework-neutral constraint, ADR-0012 for the mutation boundary,
ADR-0014 for the split read path and ADR-0018 for the realtime channel.

## Context

ADR-0007 decided *that* notes are GFM-subset Markdown stored as text, and that
`packages/editor` would hold the allowlist, the editor and the renderer used
identically by all three UI bundles. It did not say what the subset is
precisely, how the renderer stays safe, where the rules are enforced, or what
the editor's own state is.

Building Host note editing forced all four, plus a size limit the PRD never
gave. They constrain each other, so they are settled together here.

`Domain::write_note` already existed from step 2 and needed no change: it
upserts the single note, appends an immutable `note_versions` row, writes the
audit record in the same transaction, and publishes `note.changed` after the
commit. What was missing was everything around it.

## Decision 1: the editor's state is the Markdown string

The stored note is the canonical form, and the editing surface is a textarea
holding that exact text beside a live preview.

There is deliberately **no document model between the author and storage**.
Parsing to a tree and serialising it back on every keystroke would let the
application rewrite a Host's formatting - collapsing their spacing, renumbering
their list, moving their table pipes - and a note editor that edits the note
behind your back is worse than a plain one. ADR-0007 already accepted "a plain
or lightly assisted editing experience" as the price of text storage; this is
that price, paid deliberately.

A save stores exactly what was typed. `serialize` exists for content that is
*constructed* rather than typed, and callers ask for it explicitly; nothing on
the save path calls it.

The one exception is **line endings**, normalised from CRLF to LF at the Tauri
command boundary. That is a transport concern: a WebView on Windows can submit
CRLF, the domain refuses a carriage return as a control character, and a Host
should not be shown a validation error about a character they cannot see. It
also keeps stored content uniform, which export determinism will need.

## Decision 2: the subset, written down

Supported: paragraph, ATX heading (levels 1-6), unordered list with `-`,
ordered list with `1.`, GFM pipe table, link, emphasis, strong, inline code,
fenced code, hard line break.

Unsupported: images, raw HTML, blockquotes, task lists, strikethrough,
footnotes, autolinks, reference links, nested lists beyond one level, setext
headings.

Two consequences worth stating:

- **Unsupported is not invalid.** A blockquote, a task-list marker, a
  strikethrough and a footnote reference are rendered exactly as written. They
  are somebody's writing, and the right response to syntax this application
  does not interpret is to show it, not to refuse the note. A nested list
  marker becomes a sibling item at the one level that exists, so the writing
  survives and stays readable.
- **Images are literal, by a rule of their own.** `![a](url)` is emitted as
  text rather than falling through to the link parser, because a link wearing
  an image's syntax is exactly the surprise a renderer should not produce.

Line breaks follow GFM: a single newline inside a paragraph is a space, and two
trailing spaces are a hard break. Doing something else while calling the format
a GFM subset would make the name a lie.

**Ordered lists are included although the step's approved subset list omitted
them.** PRD section 13.2 and ADR-0007 both require ordered lists, and those
documents are authoritative over an implementation prompt. Dropping a construct
the product requires would have been a silent scope reduction; this is recorded
here rather than left as a difference somebody notices later.

## Decision 3: raw HTML is refused, but `<` is not

`a < b` and `2 < 3` are arithmetic. A validator that refused them would be
refusing ordinary meeting notes, so a bare `<` is text.

What is refused is a construct a browser would recognise:

| Construct | Example |
| --- | --- |
| comment | `<!-- note -->` |
| declaration | `<!DOCTYPE html>`, `<![CDATA[ ]]>` |
| processing instruction | `<?xml ?>` |
| tag, open or closing, that is actually closed | `<script>`, `</div>`, `<br/>` |

A tag needs its `>` on the same segment, which is what keeps `a<b was measured`
out of it: a name followed by prose and no bracket is not a tag.

Code is exempt. A `<script>` inside a fence or a code span is somebody writing
*about* a script tag - an ordinary thing to do in a technical meeting - and it
renders as literal text, so there is nothing to refuse.

## Decision 4: the renderer emits DOM nodes, never HTML

`renderMarkdown` returns a `DocumentFragment`. Every node is built with
`createElement`; every character of author text is set through `textContent`.
There is no HTML string anywhere in the package.

This is stronger than sanitising. A sanitiser defends a sink; this has no sink.
There is no parse-then-clean step to get subtly wrong, no bypass to discover,
and no dependency to keep patched - injection is not blocked, it is
unrepresentable.

The element allowlist is `p`, `h1`-`h6`, `ul`, `ol`, `li`, `table`, `thead`,
`tbody`, `tr`, `th`, `td`, `pre`, `code`, `em`, `strong`, `a`, `br`. No `img`,
`iframe`, `object`, `embed`, `script`, `style` or SVG - none of which the subset
can produce. The only attributes set anywhere are `href`, `target`, `rel`,
`class` and a cell's text alignment, and the only one derived from author text
is `href`, which has already been through the scheme allowlist.

The same renderer runs in the **Host's** window. Participant content is hostile
input there too (PRD 13.3), and the Host gets the same renderer as everyone
else rather than a relaxed one.

## Decision 5: link targets are parsed, not matched

Permitted schemes are `http`, `https` and `mailto` (PRD 13.1, ADR-0007).

In the browser the check is `new URL(..)` and a comparison against
`URL.protocol`. A prefix comparison is the classic way to get this wrong; the
WHATWG parser lowercases the scheme, resolves escapes, and rejects a target
that is not a valid absolute URL at all. No base is passed, so a relative
target has no scheme and cannot be allowlisted.

A target that fails renders as **literal text** rather than as an anchor or a
dropped fragment - the Host sees exactly what was written and nothing is
clickable. Anchors that are created get `target="_blank"` together with
`rel="noopener noreferrer"`.

The allowlist is a list of **protocol strings** (`http:`), never URL prefixes.
That is not cosmetic: `packages/editor` is bundled into the offline remote
form, whose build guard reads the built bytes and refuses any absolute URL in
them. An allowlist written as prefixes would break that build, and the guard
must not be weakened to accommodate this package (ADR-0009). A boundary guard
now asserts the absence of absolute URL literals in `packages/editor/src`, so
the breakage surfaces in the step that writes the code rather than the step
that bundles it.

## Decision 6: the backend is the enforcement, and it is a scanner

`crates/app-core/src/note.rs` runs the same rules inside the mutating
transaction. It checks: non-empty, at most 64 KiB of UTF-8, no recognizable raw
HTML, every link target allowlisted, and no control character other than tab
and newline.

The editor runs them too, so a Host is told while typing rather than on save.
That copy is a convenience. Content can also arrive from a transport that never
saw the editor - a participant bundle in step 8B, an import in step 10 - and
the architecture rules put validation in Rust for exactly that reason.

It is a **scanner, not a parser**. Deciding the five questions above does not
need a Markdown parser, and Step 8 does not add one: the Rust renderer belongs
to the export step, where determinism across formats is the point.

## Decision 7: 64 KiB, in bytes

The PRD sets no limit. 64 KiB is generous for a meeting note and bounded enough
that the WebView, the export and a future LAN request body stay predictable.

**Bytes, not characters**, because bytes are what SQLite stores and what an
export carries. 32769 two-byte characters is over the limit at half that many
characters, and the shared fixtures pin exactly that case.

The UI shows a character count and a byte count as a convenience. It does not
replace the check.

## Decision 8: one fixture file, two implementations

`packages/editor/__fixtures__/notes.json` holds valid and invalid cases with
the refusal reason for each. The TypeScript suite reads it; so does the Rust
suite; so will the Rust renderer when export lands. ADR-0007's requirement that
the two renderers agree becomes a failing test in whichever language drifts,
rather than a good intention.

The file is read at run time in both languages, so they genuinely read the same
bytes. Oversized cases are a repeat directive rather than 64 KiB of committed
filler.

It lives outside `packages/editor/src` because it necessarily contains absolute
URLs - it tests the link allowlist - and `src` is the directory the offline
guard's mirror scans. Keeping them apart means that guard needs no exclusion
list, which is how an exclusion list stays out of the offline guard too.

## Decision 9: version history is view-only

Every write appends an immutable `note_versions` row, numbered densely from 1,
derived inside the transaction, with `UNIQUE(note_id, version)` making a lost
race a conflict rather than a duplicate and a trigger refusing `UPDATE`. That
was all in place from step 1 and needed no change.

**Restore is deferred.** PRD section 17 says the Host can *view* history; it
does not ask for restore. Adding it would be PRD expansion, and it raises a
question the schema does not answer - what restoring a version means for a
note's links, which are not versioned. A guard asserts that no restore command,
operation or audit action exists, so it cannot arrive without the decision
being made.

**Structured note links are deferred** for the same reason. The table, its
scheme `CHECK` and its five-link triggers exist from step 1; what is deferred
is code that reads or writes the rows.

## Decision 10: last-write-wins, with an advisory notice

Concurrency is already handled by the existing model: `BEGIN IMMEDIATE`, the
version derived inside the transaction, and the unique index as the backstop.
Two writers cannot produce the same version, and the later write wins the
current content.

No `expected_version` precondition is added. It would introduce a failure mode
the PRD never asked for, and it would need a considered answer for remote
import, which is deliberately last-write-wins (ADR-0003).

The real exposure is not corruption but **silent overwrite**: the Host opens
the editor, somebody else saves, and the Host saves over it. So the Host UI
listens for `note.changed` and, when the editor is dirty, leaves the draft
exactly as it is and says so, offering *keep editing* or *discard and reload*.
Saving anyway is permitted; what was overwritten is still in history, which is
what history is for.

## Decision 11: realtime reuses `note.changed`

No new event, no second mechanism. `Domain::write_note` already publishes
`note.changed` after the commit, with audience `Participant(meeting,
participant)` - the note's owner and the Host (ADR-0018).

The event carries the note id and the version, never the Markdown. The Host
refetches through the normal command path, where the read model decides what
that audience may hold.

## Consequences

- `packages/editor` is no longer a stub, and it is framework-neutral: it
  exports parse, render-to-DOM, serialise, validate and a scheme allowlist,
  which each bundle wraps. The Host UI's wrapper is ten lines around a ref.
- A TypeScript test runner exists for the first time. Vitest, with `happy-dom`
  so the renderer's DOM guarantee is asserted against a DOM rather than a
  stand-in, and `@types/node` for the fixture loader. All three are dev-only.
  `npm run check` now runs them.
- Five new boundary guards: no HTML sink in any bundle, only the shared editor
  renders Markdown, the editor stays bundleable into the offline form, nothing
  restores a version, and note links remain schema-only.
- No migration. `notes`, `note_versions` and their constraints were already
  sufficient.
- The Rust renderer is still absent, and export (step 12) is where it lands.
  The fixtures are already waiting for it.
- Participant note editing remains **step 8B**. The domain already permits a
  participant to write their own note; what is missing is a LAN route, a body
  limit larger than the current 1 KiB, and the participant-side surface. A
  guard asserts no such route exists yet.
