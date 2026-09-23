# 0021. Remote form generation and the submission artifact

- Status: Accepted
- Date: 2026-09-23

Implements PRD sections 10, 11 and 20, and architecture rules sections 6, 7, 8
and 20. Depends on ADR-0009 for the framework-free offline bundle, ADR-0007 and
ADR-0019 for the Markdown subset, ADR-0003 for note cardinality, and ADR-0011
for identifier and storage formats.

**Supersedes ADR-0008's duplicate-correction rule** and the matching wording in
architecture rules section 12. See decision 7.

## Context

Everything a remote participant needs on the Host side already exists and has
never been used. `remote_submissions` was created in migration V1 with its
idempotency key, its resolution vocabulary and its `raw_payload` column.
`Actor::RemoteImport` exists, `authorize()` grants it exactly one operation, and
`note_versions.created_by_type` already accepts `REMOTE_IMPORT`. `app-remote` is
a 44-line skeleton and `apps/remote-form` renders four paragraphs. The
identifiers `SubmissionId` and `RemoteSubmissionId`, the constant
`SUBMISSION_SCHEMA_VERSION` and the type `SubmissionResolution` are all in place.

What was missing was the artifact itself: what the Host generates, what comes
back, and what makes the two the same thing.

This ADR settles that. It covers **generation** - the Host's side of producing
the file, and the offline form's side of producing a submission from it. The
import pipeline that consumes the submission is step 10 and gets its own
decision record; the parts of it frozen here are only those that are properties
of the artifact rather than of the pipeline.

A design inspection of the repository at `120a9c5` preceded these decisions, and
two of them exist because that inspection found the documents disagreeing with
the code. Those are decisions 6 and 7, and both are reconciliations rather than
changes of direction.

## Decision 1: one HTML file, injected into an embedded template

The generated form is a single `.html` file produced by injecting a payload into
the built `apps/remote-form/dist/index.html`, which is compiled into the binary
with `rust-embed`.

```text
apps/remote-form/dist/index.html          built by npm, offline-guard checked
        |  rust-embed, via crates/app-remote/build.rs
        v
app-remote::generate(context) -> String   one payload injected
        |
        v
src-tauri writes the String to disk
```

This is the mechanism ADR-0006 already named, where `rust-embed` is listed as
embedding "the `lan-ui` bundle **and the remote-form template**". It mirrors
`crates/app-server/build.rs` and `assets.rs` exactly, including the
`create_dir_all` that keeps the crate compiling on a fresh clone before anyone
has run `npm run build:remote`.

The alternative - generating HTML in Rust with a templating crate - was
rejected. The template is already a real bundle with a real build, a real
offline guard and a real test suite. Producing a second HTML surface in Rust
would mean two places where the form's markup lives and one of them unchecked.

`app-remote` returns a `String`. It performs no file I/O: reading and writing
files is the shell's job, and `src-tauri` does it (decision 10).

## Decision 2: the payload travels in an escaped JSON island

The template carries

```html
<script type="application/json" id="submission-context">null</script>
```

and generation replaces the `null`. The form reads it with `textContent` and
`JSON.parse`, never `innerHTML`, which keeps ADR-0009's rule that this bundle
has no HTML sink.

**The escaping is load-bearing, not incidental.** A participant named
`</script>` would otherwise close the element and the rest of the payload would
become markup. The serializer must therefore emit:

| Character | As |
| --- | --- |
| `<` | `<` |
| U+2028 line separator | ` ` |
| U+2029 paragraph separator | ` ` |

Escaping `<` unconditionally is what makes `</script` unspellable inside the
island; escaping the two separators is what keeps the JSON parseable as
JavaScript source in the fallback below. A test with a hostile participant name
and a hostile note is part of this decision, not an optional extra.

**Empirical verification is required during implementation.** Vite with
`vite-plugin-singlefile` is expected to leave a non-module
`<script type="application/json">` alone, but that is an expectation and not a
verified fact. If the island does not survive the build reliably, the fallback
is a string-literal sentinel - `const CONTEXT = '__LAN_MEETING_CONTEXT__';` -
which survives minification because string literals are not comments. Whichever
mechanism is used, **the offline guard is not weakened to accommodate it.**

## Decision 3: no credential, and nothing is signed

The generated file contains no join token, no session token, no token hash, no
join URL, no port, no host address and no secret of any kind. Architecture rules
section 20 requires this; a boundary guard will assert it.

**Cryptographic signing was considered and rejected**, and the reasoning is
recorded because "why is there no signature?" is a question this file will
attract forever.

For a signature to prove that a submission came from a form this application
generated, the form would need a signing key. The form is one HTML file that
runs offline from `file://` with no server behind it, so the key would have to
be *inside the file*. Anyone holding the file holds the key, and can therefore
forge any submission the key would authenticate. The signature would prove
"produced by someone who had a copy of the form" - which is exactly what
possession of the submission already proves. HMAC with a per-form secret fails
for the same reason.

So signing here would be cost without a property. What actually establishes
trust is stated in PRD section 12 and unchanged by this ADR:

```text
database resolution  +  Host preview and confirmation  +  app-core authorization
```

A human who knows who they sent the form to looks at the meeting, the
participant and the note, and confirms. That is the trust boundary. The
consequence worth stating plainly: **a hand-edited file naming a different valid
participant of the same meeting will import into that participant's note if the
Host confirms without reading the preview.** This is inherent to a model where a
person is the authority, and it is why the preview must show the participant
name prominently and why import must never be one click.

Transport corruption - a mail client truncating the file - is caught by JSON
parse failure and schema validation. No additional checksum is added.

## Decision 4: the form is prefilled with the participant's current note

The generation context carries the participant's existing note content, or empty
content when they have none, and the offline form opens with it in the editor.

Without this, every remote submission is a rewrite: a participant who already
wrote something on the LAN, or whose note the Host drafted, would be handed a
blank page and would silently replace it. With it, a remote submission is an
edit of what is there.

The prefilled content is **form state**. It is informational and editable, and
it is never an authorization source, never an identity source and never
evidence of anything at import time. The submission that comes back is validated
exactly as though the participant had typed every character.

This is an addition to the metadata list in architecture rules section 7, which
names identity and display fields only. Section 7's list is a floor - "setiap
generated form memiliki immutable metadata" - and the note content is not
immutable metadata: it is the field the participant is there to fill.

## Decision 5: no structured links, in the form or in the submission

The submission carries `note` as GFM-subset Markdown text and carries no `links`
array. The form offers no link fields. `note_links` stays untouched.

Structured note links were deferred by ADR-0019 decision 9 and again by
ADR-0020, both times for the same unanswered question: `note_versions` versions
*content*, and what a version means for a note's links is undecided.
`src-tauri/tests/boundaries.rs::note_links_are_still_only_a_schema` enforces
that deferral today. Introducing links through the remote form would reopen two
accepted decisions from the side door, and would do it in the one bundle whose
contract is that it is as small as possible.

Markdown already carries links, with the same three-scheme allowlist enforced by
`app-core::note`, by `packages/editor` and by the `note_links.url` CHECK. A
participant who wants to reference something writes `[label](https://...)` and
it is stored, versioned, rendered and exported exactly like the rest of their
note.

## Decision 6: architecture rules section 8 and section 9, reconciled

Two items in those sections describe a shape the code does not have. Recording
the reconciliation here, rather than editing the rules, is the convention this
repository already uses - ADR-0008 did the same when it said the rules
"previously called such a table optional; it is not."

**Section 8 lists `links` in the minimal submission shape, and section 9
requires `link count <= 5`.** Under decision 5 there is no `links` array, so
there is nothing to count. The five-link limit is not abandoned: it remains PRD
section 14's rule, it remains enforced by the `note_links_max_per_note` triggers
in migration V1, and it becomes live when structured links are built. Until
then, the section 9 checklist item is **not applicable** to a submission, and a
parser must not invent a field to validate.

**Section 9 asks "meeting OPEN?".** The domain's rule is
`Meeting::ensure_mutable()`, which permits `DRAFT` and `OPEN` and refuses
`LOCKED`. Remote import uses that rule unchanged. A stricter, import-only
lifecycle rule was considered and rejected: the LAN route, the Host command and
a remote import would then disagree about when a note may be written, and the
one that disagrees would be the one nobody tests against the others. Section 9's
"OPEN?" is read as **"the meeting still accepts mutations"**, which is what
`ensure_mutable` means and what every other note path already enforces inside
the mutating transaction.

Neither reconciliation weakens a check. The first removes a check that has
nothing to check; the second replaces a stricter-sounding phrase with the rule
the backend actually enforces everywhere else.

## Decision 7: an artifact has one identity, and its hash makes modification visible

`submission_id` is a UUIDv7 **minted by the Host when the form is generated**,
baked into the HTML, and returned by the form unchanged. The form never mints
one. It identifies *a generated artifact*, not a participant and not an attempt.

Given a submission whose `submission_id` has been seen before for this meeting
and participant:

| Case | Meaning | Outcome |
| --- | --- | --- |
| same `submission_id`, same `content_hash` | the identical file again | duplicate; refused; **no second note version** |
| same `submission_id`, different `content_hash` | the artifact's content was altered after generation | **refused**, with an actionable error |
| different `submission_id` | a different generated artifact | imported on its own terms |

**This supersedes ADR-0008**, which said that "same `submission_id` with a
different `content_hash` means the participant sent a correction", and the same
sentence in architecture rules section 12. ADR-0008's version is not wrong about
the *mechanism* - the pair still distinguishes those two situations, exactly as
that ADR required - but it is wrong about what to do with the answer.

The reason is that `submission_id` identifies an artifact, and an artifact's
content is fixed the moment the participant exports it. A file bearing a known
`submission_id` with different content is not a participant correcting
themselves; it is a file that does not match the one thing the Host can check.
Accepting it would mean the ledger records an identity whose content it cannot
vouch for, and it would make the hash decorative.

**Corrections still work, through regeneration.** A participant who needs to
resubmit asks the Host for a new form; that form carries a new `submission_id`,
and case 3 imports it. The cost is one Host action. What it buys is that every
imported artifact is exactly the artifact the Host generated, and that any
tampering or corruption between generation and import is a refusal with a
reason rather than a silent second version.

A consequence worth stating: once a `submission_id` has been imported, nothing
else bearing it can ever be imported. Detection is a lookup on
`(meeting_id, participant_id, submission_id)`, which
`idx_remote_submissions_identity` already indexes; the
`UNIQUE(meeting_id, participant_id, submission_id, content_hash)` constraint
remains the backstop for the exact-duplicate case.

## Decision 8: the canonical hash

`content_hash` is computed by the Host at import time, never by the form, over a
canonical form of the submission rather than over the file's bytes:

1. parse into the typed submission structure;
2. normalise the note's line endings - `\r\n` to `\n`, lone `\r` to `\n`;
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

Hashing the raw file bytes would have been simpler and wrong. Reformatting the
JSON, reordering its keys, or a mail gateway rewriting line endings would each
change the hash of a file whose content is identical, and the artifact would be
refused under decision 7 for a change nobody made. Step 2 is the same
normalisation `app-server::routes::write_note` already applies at the LAN
boundary, so a CRLF-mangled file and a clean one hash alike.

The exclusions matter as much as the inclusions. `submitted_at` comes from a
remote machine's clock and is untrusted; including it would make every export of
an unchanged note a different artifact. `participant_name` and `source_version`
are display context. None of the four says anything about what the note is.

`content_hash` detects duplication and modification. **It is not an authenticity
mechanism** - see decision 3.

`sha2` is required by `app-remote`. It is already a workspace dependency and
already sanctioned by ADR-0006, so no new crate enters the tree.

## Decision 9: `source_version` is advisory and needs no column

The generation context and the submission carry `source_version`: the note
version current on the Host when the form was generated, `0` when no note
existed.

It is shown to the Host during the import preview - "this form was made from
version 3; the note is now at version 5" - and it is **never** a precondition.
No comparison blocks an import. ADR-0019 decision 10 declined `expected_version`
for the LAN and the Host precisely because it "would need a considered answer
for remote import, which is deliberately last-write-wins"; this is that answer,
and it is the same one.

It gets **no database column**. It is preserved verbatim in
`remote_submissions.raw_payload`, which is the forensic record, and it can be
written into `audit_logs.metadata`, which is JSON. A column would exist only to
be queried as authority, and nothing may query it as authority.

**No migration is created for step 9 or step 10.**

## Decision 10: generation writes with `std::fs`, and no plugin is added

`src-tauri` writes the generated HTML into the application data directory with
`std::fs`, and the Host command returns the exact path. The Host UI displays the
path and the minted `submission_id`.

No `tauri-plugin-dialog`, no `tauri-plugin-fs`, and no change to
`src-tauri/capabilities/default.json`, which grants `core:default` and nothing
else. `boundaries.rs::the_window_capability_grants_nothing_beyond_the_core_defaults`
asserts the absence of `fs:`, `dialog:`, `shell:`, `http:`, `process:`, `os:`,
`sql:` and `updater:`, and that guard stands.

A native save dialog would be better for the person using it. It would also mean
two new crates, a capability the frontend has never needed, and an existing
boundary guard broken by design. The trade was made in favour of the guard: file
I/O stays in Rust, where every other privileged operation in this application
already lives, and the Host copies a path instead of picking a folder.

For **import**, step 10 investigates Tauri's core drag-and-drop path delivery,
which would give Rust a path to read with `std::fs` and may need no plugin at
all. If native path delivery proves insufficient, the fallback is pasting the
submission JSON into a field. A filesystem plugin is not added for file-picker
convenience; if implementation proves the approach impossible, that is a new
architectural decision to be approved explicitly, not a quiet `npm install`.

## Decision 11: export is a download, with an offline fallback

The form's *Export Submission* button builds the JSON, wraps it in a `Blob`,
and offers it through `URL.createObjectURL` and an anchor with `download`. None
of those is a network call, and none appears in the offline guard's forbidden
list.

Blob downloads from a `file://` origin behave inconsistently across browsers, so
a visible fallback ships alongside it: a read-only textarea containing the same
JSON, with copy-to-clipboard. A participant on a browser that refuses the
download is never stuck with a submission they cannot extract.

Both paths are entirely offline. Architecture rules section 20 is unchanged: the
submission leaves the browser only when the participant asks for it, there is no
automatic upload, and there is no network transmission of any kind.

## Decision 12: one contract, two languages, one fixture file

`packages/contracts` gains the two modules its own index has documented since the
skeleton:

| File | Shape |
| --- | --- |
| `src/form-payload.ts` | what the Host bakes into the form |
| `src/submission.v1.ts` | what the form produces and the Host consumes |

`SUBMISSION_SCHEMA_VERSION` already exists and is already imported by the form.
The parser accepts exactly version `1`; any other value is refused with a
message naming the expected and the detected version.

The two languages are held together the way ADR-0007 already holds the Markdown
subset together: a fixture file read at run time by both suites.
`packages/contracts/__fixtures__/submissions.json` carries valid and invalid
rows, each invalid row naming its rejection reason, and is consumed by a Rust
test in `app-remote` and a TypeScript test. Vitest's `include` is widened only
as far as that requires.

The form reuses `packages/editor` for validation and rendering. There is no
second validator, no second parser and no second renderer, and the offline
guard's constraints on that package are the reason it was written
framework-neutral in the first place (ADR-0009).

## Decision 13: older forms stay valid

Generating a new form for a participant does not invalidate the forms generated
before it. Each carries its own `submission_id` and each remains importable
until the meeting is locked or the participant leaves the roster.

No revocation table, no expiry column, no invalidation flag and no migration. A
form that has gone stale is refused for a reason that already exists - the
meeting is locked, the participant is gone, or the artifact was already
imported - and the Host confirms every import in any case.

## Consequences

- `crates/app-remote` stops being a skeleton. It gains `sha2` and `rust-embed`,
  both already in the tree, and a `build.rs` mirroring `app-server`'s.
- `apps/remote-form` gains `@lan-meeting/editor` and becomes a real form. Its
  built size grows from 1.39 kB by whatever the editor costs; the offline
  guard's 1.5 MB budget is not close.
- **The generated artifact is checked by nothing today.** The npm guard reads
  the *template*. Step 9 adds a Rust test applying the same forbidden-pattern
  list, plus the credential check from decision 3, to generation output. The npm
  guard itself is not touched.
- ADR-0008's duplicate-correction rule and architecture rules section 12 are
  superseded on one point (decision 7). The table, its key and its resolution
  vocabulary are unchanged.
- `SubmissionResolution` keeps four values in the schema and uses two.
  `REJECTED_DUPLICATE` and `REJECTED_INVALID` are **not written in the MVP**:
  rejections are reported to the Host and leave no ledger row, because a
  rejection row would have to be written in its own transaction after the import
  transaction rolled back, and because the composite foreign key to
  `participants(meeting_id, id)` makes a row impossible for the rejections that
  matter most - an unknown meeting or an unknown participant. Revisiting that
  means revisiting the ledger schema, deliberately.
- No migration. `notes`, `note_versions`, `audit_logs` and `remote_submissions`
  were already sufficient, and `audit_logs.action` and `target_type` are free
  text with only a non-empty CHECK, so new audit actions need no schema change.
- `authz` is untouched. `Actor::RemoteImport` already has exactly the authority
  an import needs, and `Operation::WriteNote` already covers it.
- `crates/app-remote` must never construct `Actor::RemoteImport`. It parses
  untrusted input; the crate that reads the file is not the crate that produces
  authority. The actor is built in `src-tauri` from the Host-selected meeting
  and the database-resolved participant, after validation and after
  confirmation. A boundary guard will assert this.
- A participant id inside a submission is a **candidate**, resolved by lookup.
  `participant_name` is displayed for human verification and is never a match
  key, which is architecture rules section 10 and section 21 restated for this
  artifact.
- The remote form is the first surface in this application that a participant
  edits with no backend behind it. Everything it enforces is a courtesy; every
  rule is re-run by `app-core` inside the mutating transaction at import.
