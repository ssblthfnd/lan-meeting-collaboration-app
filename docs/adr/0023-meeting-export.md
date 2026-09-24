# 0023. Meeting export: lifecycle, audit and the Rust renderer

- Status: Accepted
- Date: 2026-09-24

Implements PRD sections 6, 16, 20, 21 and 25.4, and architecture rules
sections 17, 19, 19.1 and 21. Depends on ADR-0004 for the PDF deferral,
ADR-0007 and ADR-0019 for the canonical Markdown subset and the "the Rust
renderer belongs to the export step" deferral, and ADR-0021 decision 10 for
the filesystem-write precedent this reuses.

A read-only design inspection, followed by an explicit resolution of eight
material ambiguities (E-1 through E-8) and four corrections to that
resolution, preceded implementation. This ADR records the decisions that are
either new architectural surface or a deliberate departure from an existing
pattern; it does not restate every detail already settled by the design
freeze.

## Context

Step 12's scope is Phase 1's remaining MVP item: Markdown, TXT and AI Context
export of a meeting record. `crates/app-export` existed only as an empty
skeleton (one placeholder error variant, no renderer). Two questions could
not be answered by inspecting existing code alone, because nothing like them
existed yet:

1. PRD section 16 lists "export generated" as an audit-trail example, and
   PRD section 6 says `EXPORTED` "does not have to be a database status...
   export can be recorded via the audit log" - but every existing audit
   write in `app-core` happens as a side effect of a *state* mutation
   (`meetings`, `participants` or `notes` changing). Export changes no
   database state at all; the thing to record is that a file was written
   somewhere the database cannot see.
2. Notes are stored as GFM-subset Markdown text with no Rust parser -
   `app_core::note` is deliberately a scanner, not a parser (ADR-0019
   decision 6). TXT and AI Context need to strip Markdown syntax down to
   plain text, and no such conversion existed in Rust; only a TypeScript
   test oracle (`packages/editor/tests/support.ts::plainTextOf`) and a
   fixture field it is checked against.

## Decision 1: an audit-only mutation, authorized and transactional like every other one

`Domain::record_export(actor, meeting_id, format)` writes exactly one
`audit_logs` row - action `meeting.exported`, target the meeting, metadata
`{"format": "markdown"|"txt"|"ai_context"}` - and nothing else. It is the
first `Domain` method whose only effect is an audit row; every other method
also changes a `meetings`, `participants` or `notes` row in the same
transaction.

It is not a new *kind* of trust boundary: it still authorizes through
`authorize(actor, meeting_id, Operation::GenerateExport)`, still runs inside
one `BEGIN IMMEDIATE`, and still re-reads the meeting's lifecycle status
inside that transaction (`Meeting::ensure_exportable`) rather than trusting a
caller's earlier check. `Operation::GenerateExport` is `HOST_ONLY`, exactly
like `LockMeeting`.

**A `DomainEvent` is deliberately not published.** Every existing mutation
publishes exactly one; this is the first exception. Nothing in this
application needs to be told an export happened - no LAN participant has any
reason to know a file was written to the Host's own disk, and there is no
second Host window (the same reasoning Step 11 already applied when it added
no event subscription for `meeting.locked`). Adding one anyway "for
consistency" would be a notification with no reader.

## Decision 2: the filesystem write and the audit write are not atomic, and no mechanism pretends otherwise

The sequence is:

```text
Host command generate_export
  -> HostState::check_export_eligible   (read-only: exists, authorized,
                                          not DRAFT; no BEGIN IMMEDIATE)
  -> app_export::render_*               (pure: no DB, no filesystem, no auth)
  -> std::fs::write                     (the Tauri boundary)
  -> Domain::record_export              (re-authorizes, re-checks lifecycle,
                                          writes exactly one audit row)
```

Two lifecycle checks run, on purpose. `check_export_eligible` exists so a
`DRAFT` meeting never reaches the renderer or the filesystem at all - it is
the same "ask early to avoid wasted work" shape `generate_remote_form`
already uses for its own `LOCKED` courtesy refusal (ADR-0021), at the same
layer (`HostState`, not `Domain`; see decision 3). `Domain::record_export` is
what actually *enforces* the rule, inside its own transaction, exactly as
architecture rules section 15 requires. The lifecycle is monotonic
(`DRAFT -> OPEN -> LOCKED`, never backward - `Meeting::ensure_transition`
permits no other edge), so a meeting the first check found `OPEN` or
`LOCKED` cannot have become `DRAFT` again by the time the second check runs.

If `Domain::record_export` fails **after** the file has already been written
successfully, the file remains on disk. `std::fs` does not participate in a
SQLite transaction, and no distributed-transaction mechanism - two-phase
commit, a write-ahead journal spanning both, a compensating delete - is
introduced to fake one. The command reports a persistence error naming this
specific case (`export_recorded_failed`), distinct from an ordinary
filesystem-write failure, so a Host is told which half succeeded. This is
accepted as the one non-atomic edge in this design, the same way every other
file-writing feature in this codebase already accepts that a full disk or a
permission error is simply reported, not rolled back into a state that never
existed.

## Decision 3: the eligibility gate lives in `HostState`, not on `Domain`

`app-core`'s `Domain<D: Database>` holds only a write-capable connection
(`D: Database`, whose `transaction` always opens `BEGIN IMMEDIATE` -
confirmed by reading `app-db`'s implementation). There is no read-only path
into a transaction, and `app-core` has no dependency on `app-db`'s
`HostQueries` at all - taking one would be circular, since `app-db` already
depends on `app-core` for its types.

A literal `Domain::check_export_eligible` reading through "the existing
HostQueries/read-pool pattern" is therefore not implementable without either
adding a new read-only method to the `Database`/`DomainTx` port (new
abstraction, for one caller) or moving the check to the layer that already
owns `HostQueries` and already has this exact shape of check.
`HostState::check_export_eligible` does the latter: it reads
`HostQueries::meeting()` (no transaction, the read pool, ADR-0014), calls the
free function `app_core::authz::authorize` (which needs no database access
at all - it is a pure decision over the `Actor` and `Operation`), and checks
`status != Draft` itself, mapping a `DRAFT` meeting onto the same
`DomainError::MeetingNotOpen` / `HostErrorKind::MeetingNotOpen` the rest of
the stack already uses.

This preserves every behavioural requirement the design freeze set - a
cheap, read-only, `BEGIN IMMEDIATE`-free gate that keeps a `DRAFT` meeting
away from the renderer and the filesystem - without inventing a new port
method for a single caller. `Meeting::ensure_exportable` still exists on the
domain `Meeting` type and is still what `Domain::record_export` calls inside
its transaction; only the *first*, courtesy check sits one layer up, at
exactly the layer `generate_remote_form`'s own courtesy check already lives
at.

## Decision 4: `MeetingNotOpen` is reused for the `DRAFT` refusal, message wording accepted as-is

`Meeting::ensure_exportable` accepts `OPEN` and `LOCKED` and refuses only
`DRAFT`, reusing `DomainError::MeetingNotOpen { meeting_id, detected }`
rather than adding a variant. The existing message text - "is not open to
participants: expected status OPEN, detected {detected}" - was written for
the join/claim case, where only `OPEN` is ever accepted. For export, `OPEN`
is still one of the two accepted statuses, so the message remains factually
accurate; it simply does not separately name `LOCKED` as also acceptable.

**This imprecision is accepted rather than fixed with a new variant**,
because the design freeze was explicit that no new `HostErrorKind` should be
added unless an existing one is technically insufficient, and this one is
not insufficient - only slightly generic. A future step is free to give
export its own message text if this is ever reported as confusing in
practice; it is not blocking today.

## Decision 5: a bounded, hand-written Rust parser - not a CommonMark crate, not a JS engine

`crates/app-export::markdown_ast` parses exactly the subset ADR-0007 already
allowlists (paragraph, ATX heading 1-6, one flat level of ordered/unordered
list, GFM pipe table, link, emphasis, strong, inline code, fenced code, hard
line break), mirroring `packages/editor/src/parse.ts` construct for
construct. Everything outside the subset renders as plain text, exactly as
the TypeScript side already treats it - this parser never refuses input; the
content it receives has already passed `app_core::note::validate`.

`crates/app-export::plain_text` walks that AST into the same plain-text
contract `packages/editor/tests/support.ts::plainTextOf` already pins on the
TypeScript side: a block-level construct ends with one line break, a table
cell ends with one space, blank-line runs collapse to one, the result is
trimmed. `crates/app-export/tests/plain_text_fixtures.rs` checks the Rust
renderer against every fixture in `packages/editor/__fixtures__/notes.json`
that records a `text` field - the same fixture file the TypeScript suite and
`app-core`'s validator suite already read, closing the gap where the Rust
side previously ignored that field entirely.

**A third-party Markdown/CommonMark crate was rejected.** A general parser
would accept constructs outside the frozen subset and would not agree with
`parse.ts`'s specific behaviour (its documented "less-than-in-prose"
regression, its unclosed-backtick handling, its image-is-always-literal
rule) without extensive, fragile reconfiguration - which is exactly "a new
Markdown dialect" the design freeze forbade. **Embedding a JS engine to reuse
`packages/editor` directly was also rejected** - `app-export` is a pure Rust
crate with no runtime dependency of that kind anywhere else in this
codebase.

## Decision 6: `app-export` depends on nothing but the standard library

Every renderer (`render_markdown`, `render_txt`, `render_ai_context`) is a
pure function from a plain `ExportDocument` - meeting metadata, a roster
projection, per-participant latest-note text, a three-fact timeline - to a
`String`. The crate has no dependency on `app-core`, `app-db`, `thiserror` or
any other crate: the skeleton's original `Cargo.toml` listed `app-core` and
`app-db`, and both are removed, because nothing in the finished crate uses
them.

This makes "app-export cannot reach into `HostQueries` or open the database"
true by construction rather than by convention: there is no dependency edge
through which it could. `src-tauri::host::assemble_export_document` is the
only place `HostQueries` results become an `ExportDocument`; the projection
deliberately narrows the roster to `name`, `department`, `position` and
`meeting_role` only, carrying no participant id, claim status or session
state into the document at all.

## Decision 7: note headings are never re-levelled

A participant's note may contain its own Markdown headings, including a
document-root-level `#`. The Markdown renderer emits stored note content
**verbatim** beneath a participant's `### <name>` section heading, and does
not adjust a note's own heading level to nest visually underneath it.

**Re-levelling was rejected.** It would mean parsing and rewriting canonical
note content for a cosmetic document-hierarchy purpose, which contradicts
the same principle ADR-0019 already states for note storage: content is
somebody's writing, and the application does not silently edit it. The
consequence is stated plainly rather than glossed over: a note's own `#`
heading becomes, nominally, the same Markdown heading level as the
document's title. A tool or person parsing the exported file must treat
embedded note content as verbatim source material, not as a structurally
normalised subtree of the document.

## Decision 8: AI Context performs no heading recognition of any kind

AI Context is TXT's plain-text rendering, preceded by one fixed, static
preamble sentence that names only the nature of the export process, never a
fact about the meeting:

> "The following is a structured export of a meeting record. It has been
> formatted only; nothing has been summarized, inferred, or added."

An earlier design draft considered a small allowlist of heading text
("Discussion", "Decision(s)", "Action Item(s)") that would be preserved
specially. **That allowlist was rejected before implementation**, in favour
of a simpler and stronger rule: no heading is ever inspected for its text at
all, only its block type. PRD section 21's requirement to "distinguish"
discussion from decision from action-item content is satisfied as a side
effect of faithful, non-lossy structural rendering - if an author already
separated those with their own headings or paragraphs, that separation
survives because nothing here merges or reflows block boundaries. Nothing
in this renderer ever asks "is this heading a decision?", which is what
keeps it formatting rather than interpreting (architecture rules section
19.1).

## Decision 9: the meeting timeline's "Opened" fact is deterministic by construction

The exported timeline states `created_at` (from `meetings`, no audit read),
`opened_at` (the earliest `audit_logs` row with `action = 'meeting.opened'`
for the meeting) and `locked_at` (from `meetings`, present only when
`LOCKED`).

`Meeting::ensure_transition` permits `Draft -> Open` exactly once and never
back - re-opening an already-open meeting is refused as `InvalidTransition`,
not treated as a harmless repeat. A `meeting.opened` audit row is therefore
written **at most once** per meeting, and every meeting this feature ever
renders a timeline for (`OPEN` or `LOCKED`, per decision 4) has necessarily
passed through it exactly once. Selecting the earliest such row is
future-proofing rather than a hedge against a present possibility: the
lookup reuses `HostQueries::audit_entries`, which is already ordered
`created_at ASC, id ASC` by an existing index
(`idx_audit_logs_meeting_time`), so the first matching row in that
already-ordered result is the earliest one. **No new query, no new index and
no migration were added for this.**

## Decision 10: exports live in their own `app_data_dir` subdirectory, written with `std::fs`, no dialog

`EXPORT_DIRECTORY = "exports"`, resolved from `app.path().app_data_dir()`
exactly as `REMOTE_FORM_DIRECTORY` already is (ADR-0021 decision 10). No
`tauri-plugin-dialog`, no `tauri-plugin-fs`, no change to
`src-tauri/capabilities/default.json`; the existing boundary guard
(`the_window_capability_grants_nothing_beyond_the_core_defaults`) still
passes unmodified. The Host window is never given a path to choose and
never sees or supplies a filesystem path beyond the resulting absolute path
returned in the response.

The filename - `{meeting-slug}-{format}-{id-tail}.{md|txt}` - reuses the same
slugging discipline `remote_form_file_name` already applies (ASCII
alphanumeric runs joined by single dashes, capped length, a `"meeting"`
fallback for an empty slug), duplicated rather than extracted into a shared
helper so `remote_form_file_name` itself is untouched. The `id-tail` is the
last twelve hex characters of a freshly minted UUIDv7 (`uuid::Uuid::now_v7`,
already a workspace dependency used throughout `app-core` - no new crate
enters the dependency graph), guaranteeing every export is its own file: an
export never overwrites an earlier one of the same meeting and format.

## Decision 11: `generated_at` never appears inside exported content

The export DTO carries `generated_at`, exactly as `RemoteFormGeneratedDto`
already does; the rendered file bytes never reference wall-clock time. Two
exports of the same, unchanged `LOCKED` meeting are byte-for-byte identical -
verified directly by
`export_commands.rs::identical_source_state_produces_byte_identical_markdown`.
Embedding a timestamp "because it is available" was rejected: a record whose
defining property is that it does not change when nothing about the meeting
changed would otherwise differ on every regeneration for no reason a reader
could act on.

## Consequences

- `crates/app-export` stops being a skeleton. It gains `markdown_ast`,
  `plain_text` and `document` modules and drops its dependency on
  `app-core`, `app-db` and `thiserror`; its only dependency is a dev-only
  `serde_json`, for the shared-fixture cross-check.
- `app-core` gains `Operation::GenerateExport`, `AuditAction::MeetingExported`
  (`"meeting.exported"`), `ExportFormat`, `ExportRecorded`,
  `Meeting::ensure_exportable` and `Domain::record_export`. No new
  `DomainError` variant, no new `DomainEvent` variant, no schema change.
- `src-tauri` gains `HostState::generate_export`,
  `HostState::check_export_eligible`, `HostState::assemble_export_document`,
  the `generate_export` Tauri command, `ExportGeneratedDto` and
  `EXPORT_DIRECTORY`. The registered-command / gateway parity guard moves
  from 29 to 30.
- `packages/contracts` gains `'meeting.exported'` on `AuditAction` and the
  `ExportFormat` / `ExportGenerated` types. The pre-existing gap where
  `'remote_submission.imported'` is missing from that same union - found
  during this step's design inspection - is left untouched; it predates
  Step 12 and is not this decision's to fix.
- `apps/host-ui` gains an `ExportPanel` component and an `Export` tab on
  `MeetingDetailView`, following `RemoteFormPanel`'s existing
  button-then-result pattern. The tab is not hidden based on `meeting.status`
  as a substitute for the backend's own lifecycle check; the UI is not the
  security boundary.
- No migration, no new Tauri capability, no CSP change, no new LAN route, no
  new WebSocket message. `uuid` gains one new dependency edge in
  `src-tauri`'s own `Cargo.toml`, to a crate already resolved elsewhere in
  the workspace; no new crate enters the lockfile.
