# 0022. Remote submission import

- Status: Accepted
- Date: 2026-09-23

Implements PRD sections 11 and 12, and architecture rules sections 9, 10, 11 and
12. Depends on ADR-0003 for note cardinality, ADR-0007 and ADR-0019 for the
Markdown subset, ADR-0011 for storage formats, ADR-0012 for the mutation
boundary, ADR-0014 for the split read path, ADR-0018 for the event channel, and
ADR-0021 for the artefact this step consumes.

**Supersedes the remaining duplicate-semantics wording of ADR-0008**, and
narrows one part of ADR-0021 decision 7. See *Supersession* at the end. ADR-0008
itself is left exactly as written.

## Context

Step 9 built the artefact: the Host generates a self-contained offline form, a
remote participant fills it in and exports a submission file. Nothing reads one
back. This decision record is the other half.

A design inspection of the repository at `e867bdc` preceded it, and three of the
step's hardest-looking problems turned out to be already solved:

1. **File import needs no new capability.** `core:default` already resolves to
   `core:event:default`, which grants `allow-listen`; `dragDropEnabled` is on by
   default; and a Rust-side drag-drop handler needs no permission at all,
   because the ACL governs what the *renderer* may reach over IPC.
2. **Cross-meeting import is structurally impossible.** `participants.id` is a
   global `PRIMARY KEY`, so a participant id belongs to exactly one meeting.
3. **Import attribution is automatic.** `DomainTx::insert_note_version` and
   `insert_audit` already take `actor_type` and `actor_id` from the
   authorization proof, so an `Actor::RemoteImport` proof writes correct
   `REMOTE_IMPORT` history and audit rows with no change to either method.

What is left is the part that has to be got right: one atomic transaction, and a
trust boundary that never reads authority out of the file.

## Decision 1: the Host selects the meeting; the file only agrees

A submission names a meeting. That name never selects the destination.

```text
Host-selected meeting  +  submission's candidate meeting_id  ->  must be equal
```

If they differ the import is refused before the Host is asked to confirm
anything. The API is shaped so the renderer cannot express the other
arrangement: both the preview and the confirm command take the Host-selected
meeting id, and there is no call that imports "wherever the file says".

## Decision 2: every identifier in the file is a candidate

`meeting_id`, `participant_id` and `participant_name` are **untrusted input**
(architecture rules section 10). They are inputs to a lookup, never authority.

| Field | Used for | Never used for |
| --- | --- | --- |
| `meeting_id` | equality against the Host's selection | choosing a meeting |
| `participant_id` | a meeting-scoped lookup in `participants` | identifying who may be written |
| `participant_name` | display, so a human can verify | matching a participant |

The participant the note is written for is the **row the database returned**.
`participant_name` is shown beside the database's own name so a mismatch is
visible; it is a warning to a person, and it is written nowhere.

## Decision 3: `Actor::RemoteImport` is built from resolved rows, in `src-tauri`

```text
meeting resolved in the database
  -> submission.meeting_id equals the Host's selection
    -> participant resolved, scoped to that meeting
      -> Host confirms explicitly
        -> Actor::RemoteImport { meeting_id: <from the row>, participant_id: <from the row> }
```

`crates/app-remote` parses and hashes untrusted input and **must never construct
an actor**: the crate that reads the file is not the crate that produces
authority. A boundary guard asserts it constructs no `Actor::` at all, and a
second guard will assert the construction site is `src-tauri/src/host.rs` and
nowhere else.

Even if that discipline were broken, `authorize()` independently refuses a
target participant that is not the actor's own, and refuses any meeting that is
not the actor's own - both inside the mutating transaction. The transport cannot
reach around either.

## Decision 4: preview and confirmation are two operations, and the second trusts nothing

**Preview is read-only.** It parses, hashes, resolves, inspects the ledger and
loads the current note, entirely through `HostQueries` on the read-only
connection pool - which is opened `SQLITE_OPEN_READ_ONLY`, so a write through
that path is refused by SQLite rather than by discipline.

**Confirmation repeats every security-critical check against current state.**
Between the two, a meeting can be locked, a participant removed, another
submission imported, or the note changed by somebody on the LAN. The preview's
conclusions - eligibility, the resolved ids, the hash, the duplicate verdict -
are **advisory display**, and none of them is carried into the write as
authority.

The artefact is held in a small Rust-side pending-import state, not passed back
through the window. That serves two ends at once: the renderer cannot hand back
something the Host never saw, and the renderer never names a filesystem path.

## Decision 5: one transaction, and no second one

A successful import is exactly **one** `BEGIN IMMEDIATE` transaction:

```text
BEGIN IMMEDIATE
  load meeting
  authorize the RemoteImport actor     -- inherited from the step 8 note write
  ensure_mutable                       -- the lock, re-read here and nowhere else
  validate the note content
  resolve the participant in this meeting
  check submission identity            -- duplicate / modified / cross-participant
  upsert the single note
  append note_versions
  insert remote_submissions
  insert audit_logs
COMMIT
  publish note.changed
```

Six invariants, all of them inside the transaction except the last:

1. the lock is checked inside the transaction;
2. actor authorization is checked inside the transaction;
3. participant confinement is checked inside the transaction;
4. submission identity is checked inside the transaction;
5. note, version, ledger row and audit entry commit together or not at all;
6. the event is emitted only after the commit returns.

### Why authorization precedes the lock

That order is **not a decision this record makes**. It is the sequence
`Domain::write_note` has shipped with since step 8, and the shared helper below
preserves it rather than reopening it.

It has to. `app-server` maps `Forbidden` and `MeetingLocked` onto different
answers, so swapping the two would change what an unauthorized LAN participant
learns about a locked meeting - from "you may not" to "this meeting is not
open". A step 10 refactor is not the place to change what a step 8 transport
already tells somebody.

What matters for import is unaffected, and is what the invariants above state:
the lock is enforced **inside the same transaction**, before any note mutation
and before any submission-identity decision.

### Why the gate precedes submission identity

Everything down to and including the participant lookup is a **gate**, and it
runs in full before the first ledger lookup. That ordering is load-bearing:

- a locked meeting must be refused **as locked**, even when the file offered is
  also a duplicate or a modified artefact;
- an actor with no standing must be refused **as unauthorized**, even when the
  artefact it carries is one this application has already seen.

**Artefact identity is never an authorization shortcut, and never a lifecycle
one.** A Host told "already imported" about a locked meeting would go looking
for a problem with the file that is not there.

### The shared helper

**Calling the public `Domain::write_note` and then writing the ledger is
prohibited.** It is two transactions, and it can leave a note written with no
ledger row - which is idempotency quietly broken.

The accepted structure is **private in-transaction helpers extracted from
`write_note`**, performing the existing step 8 sequence unchanged, with
`write_note` and `import_remote_submission` as their two callers:

| Helper | Owns |
| --- | --- |
| the gate | meeting load, authorization, the lock, content validation, membership - and returns the authorization proof |
| the write | the upsert, the version derivation, the `note_versions` row |

An import runs its identity lookups *between* the two. The ledger row is
inserted with the *same* proof, so `proof.meeting_id()` scopes it and no caller
can supply a different meeting.

Note-version derivation, lock semantics and the authorization sequence must
exist in **one** implementation. Two would be two places for them to drift, and
the things they would drift on are exactly the ones that must not.

## Decision 6: submission identity, and the three ways it is refused

`submission_id` is minted by the Host when a form is generated (ADR-0021
decision 7). Given a submission for the Host-selected meeting:

| Condition | Verdict |
| --- | --- |
| `(meeting, participant, submission_id)` seen, same `content_hash` | **DUPLICATE** - refused |
| `(meeting, participant, submission_id)` seen, different `content_hash` | **MODIFIED_ARTIFACT** - refused |
| `(meeting, submission_id)` seen under a **different** participant | **CROSS_PARTICIPANT_ARTIFACT** - refused |
| otherwise | **NEW** - imported |

All three checks run **inside the domain transaction**, not only in the preview.
A check that lives only in a preview is a race, and a check that lives only in
the UI is not a check.

The third row extends ADR-0021 decision 7, which keyed detection per
participant. An artefact whose `participant_id` was edited produces a different
canonical hash - `participant_id` is a hashed field - so it would have missed the
per-participant lookup entirely and imported into someone else's note. The
`(meeting_id, submission_id)` lookup closes that: a generated artefact belongs to
one participant, and seeing it under another is evidence the file was altered.

**None of these is a correction mechanism.** A participant who needs to resubmit
asks the Host for a **new form**, which carries a new `submission_id` and imports
as NEW. There is no Host "replace" action for remote submissions in this step.

The `UNIQUE(meeting_id, participant_id, submission_id, content_hash)` constraint
remains the backstop for a race the in-transaction lookups cannot see; a
violation of it is reported as a duplicate, not as a failure.

## Decision 7: rejected submissions are not persisted

For the MVP, a refusal writes **nothing**: no `remote_submissions` row, no
`note_versions` row, no audit entry, no note change. Malformed, unsupported,
mis-identified, locked, removed-participant, duplicate, modified and
cross-participant submissions are all reported to the Host and leave no trace in
the database.

There is no second rejection-write transaction. Two reasons, and the second is
structural:

- a rejection row written inside the import transaction would roll back with it;
- the composite foreign key to `participants (meeting_id, id)` makes a row
  **impossible** for exactly the rejections that would matter most - an unknown
  meeting or an unknown participant.

`REJECTED_DUPLICATE` and `REJECTED_INVALID` therefore stay in the schema's
`CHECK` and unused. Using them means revisiting the ledger schema deliberately,
in a later design.

Rows written by successful imports are sufficient to recognise an artefact that
has already been processed, which is what idempotency requires.

## Decision 8: `resolution` is `IMPORTED` or `REPLACED`

| Value | Meaning |
| --- | --- |
| `IMPORTED` | the participant had no note; this import created it at version 1 |
| `REPLACED` | the participant had a note; its content was replaced, at version N+1 |

That maps directly onto what the note upsert already reports, and it satisfies
the schema's `CHECK ((resolution IN ('IMPORTED','REPLACED')) = (note_version IS
NOT NULL))`. No third value is invented.

## Decision 9: the lifecycle rule is the domain's

Import uses `Meeting::ensure_mutable()` unchanged: `DRAFT` and `OPEN` permitted,
`LOCKED` refused. No transport-specific lifecycle rule, because the LAN route,
the Host command and an import would then disagree about when a note may be
written, and the one that disagreed would be the one nobody tested against the
others.

The check is re-read inside the write transaction, in the gate, before any note
mutation and before any submission-identity decision. A meeting locked between
preview and confirmation makes the import fail atomically, leaving no note,
version, ledger row or audit entry.

## Decision 10: `source_version` never blocks

It is the note version current on the Host when the form was generated, or `0`.
It is shown in the preview - "this form was made from version 3; the note is now
at version 5" - and it decides nothing.

A stale form imports successfully and produces a new version. The superseded
content stays in `note_versions`, which is what history is for. This is
**last-write-wins**, consistent with ADR-0003 and with ADR-0019 decision 10,
which declined `expected_version` for every write path precisely so that remote
import would not need a different answer.

It gets no database column. It is preserved verbatim in
`remote_submissions.raw_payload` and written into `audit_logs.metadata`.

## Decision 11: the canonical hash is the one Step 9 already built

`app_remote::content_hash` over `app_remote::canonical_form`, with the
`lan-meeting/remote-submission/v1` domain separator, the length-prefixed note,
and the four excluded fields. **No second hash algorithm is written.** The
import side calls the same function the fixtures pin, and
`packages/contracts/__fixtures__/submissions.json` continues to hold the Rust
and TypeScript implementations to the same bytes.

The hash answers *"is this the same artefact I have already seen?"* It is
**not** authentication, and it is never treated as any.

## Decision 12: nothing is signed

No HMAC, no signature, no private key, no shared secret, no embedded credential,
no cryptographic identity. ADR-0021 decision 3 gives the reasoning in full: a key
inside a file that runs offline is a key everyone holding the file has, so a
signature would prove only what possession already proves.

Possession of a generated form is not proof of participant identity. The trust
boundary is, and remains:

```text
database resolution  +  Host preview and confirmation  +  app-core authorization
```

## Decision 13: what happens to each awkward case

Frozen behaviour, so none of it is decided at implementation time:

| Situation | Behaviour |
| --- | --- |
| participant id exists, name differs | **warning** in the preview; import permitted; the database name wins and is what is stored |
| participant id does not exist | **hard error** - refused |
| participant belongs to another meeting | **hard error**; also structurally impossible, since a participant id belongs to exactly one meeting |
| submission meeting differs from the Host's selection | **hard error** - refused before confirmation |
| displayed meeting metadata differs from the database | **warning**; the database is authoritative and nothing from the file is stored |
| `source_version` is stale | **not an error** - imports, producing a new version |
| timestamps malformed or implausible | **not an error** - display only, stored verbatim in `raw_payload`, excluded from the hash |
| participant removed from the roster | **hard error** - refused |
| participant's LAN session revoked, still on the roster | **imports normally** |

The revoked-session row deserves its reason stated. Session revocation governs
*LAN access*; remote participation is session-less by design, and most remote
participants never had one. Making revocation block an import would couple two
unrelated concepts and would break the ordinary case.

## Decision 14: audit and events reuse what exists

One audit row per successful import:

| Column | Value |
| --- | --- |
| `actor_type` | `REMOTE_IMPORT` - from the proof, automatically |
| `actor_id` | the participant - from the proof, automatically |
| `meeting_id` | the Host-resolved meeting - from the proof |
| `action` | `remote_submission.imported` |
| `target_type` / `target_id` | `note` / the note id |
| `metadata` | `participant_id`, `submission_id`, `content_hash`, `source_version`, `note_version`, `resolution` |

`audit_logs.action` and `target_type` are free text with only a non-empty
`CHECK`, so this needs no schema change, and `AuditTarget::Note` needs no new
variant. The append-only triggers are untouched. No credential, no raw payload
and no note body goes into metadata.

Events reuse `DomainEvent::NoteChanged` unchanged - identifiers, a version and a
timestamp, never the Markdown. Its audience is already the note's owner and the
Host. **No new event variant**, which also leaves the exhaustive audience table
and its variant count alone. It is published by `Domain` after the commit
returns; a failed import never reaches that line, so no event is emitted.

## Decision 15: the raw payload is the original bytes

`remote_submissions.raw_payload` stores the **exact submitted JSON, verbatim** -
not a re-serialisation. That is what the column was created for: *"the external
input, verbatim, for forensics… deliberately not required to be valid JSON."*

Canonicalisation changes nothing about what is stored; canonical bytes exist only
to be hashed and compared. The generated HTML artefact is never stored. The
payload must be valid UTF-8 to be a `TEXT` value, and a file that is not is
refused at read time as malformed.

## Decision 16: 256 KiB, checked twice, before anything is read whole

| Limit | Where | Value |
| --- | --- | --- |
| complete submission JSON | `src-tauri`, at read time | **256 KiB** |
| note content | `app-core::note`, in the transaction | 64 KiB, unchanged and authoritative |
| `raw_payload` | bounded by the file limit above | no database constraint |

The size is checked **before** the file is read, and the result of the read is
checked again, so a file that grows between the two cannot get past the bound.
Nothing reads a file of unknown size into memory.

Filenames are never parsed and never used as identity (architecture rules
section 21). The renderer never supplies a path at all.

## Decision 17: the file arrives by drag and drop, with a paste fallback

Tauri's core drag-drop delivers the dropped path to **Rust**. The path is kept in
a small Tauri-managed pending-import state; the renderer is told a file is
waiting and calls preview with a meeting id and no path. A renderer that cannot
name a path cannot read an arbitrary file, which is the same principle Step 9
applied to writing.

The drag-drop handler delegates to a separately testable function rather than
holding logic inside `run()`, which no integration test can execute.

A paste-JSON field is the fallback, for a Host on a remote desktop or using
assistive technology. It feeds the same pipeline with no filesystem involved.

**No `tauri-plugin-fs`, no `tauri-plugin-dialog`, and no change to
`src-tauri/capabilities/default.json`.**

## Decision 18: no migration

`V1` and `V2` remain the only migrations. Verified column by column: the ledger
row, its idempotency key, the hash format, the resolution/version coherence
check, import attribution, the new audit action and metadata, the raw payload and
the cross-meeting foreign key are all already expressible. `V1` anticipated this
step.

## Supersession

**ADR-0008, decision 2** stated that a submission bearing a known `submission_id`
with a different `content_hash` "means the participant sent a correction". That
reading was already replaced by ADR-0021 decision 7, which refuses it as a
modified artefact; this ADR records the same verdict for the import pipeline that
acts on it, and adds the cross-participant case ADR-0021 did not cover.

**ADR-0021, decision 7** keyed artefact identity on
`(meeting, participant, submission_id)`. Decision 6 above widens it: a lookup on
`(meeting, submission_id)` also refuses an artefact that reappears under a
different participant. Everything else in ADR-0021 decision 7 stands.

**ADR-0008 is not edited.** The ADR index states that a decision which changes
gets a new record rather than an edit that erases the history, and ADR-0008's
account of *why* the ledger exists, and of its key, remains correct and in force.

## Consequences

- `app-core` gains one public operation, two private in-transaction helpers -
  the gate and the write - two port methods, one audit action and three error
  variants. `authz.rs` and `actor.rs` need **no change**: the authorization rule
  for import has existed and been tested since step 2.
- `Domain::write_note` is refactored onto the same two helpers. That is
  behaviour-preserving and required: two implementations of the authorization
  sequence, the lock re-read or version derivation would be two places for them
  to disagree.
- `app-remote` gains at most a size-limit error. Parsing and hashing already
  exist and are fixture-locked.
- `crates/app-server` and `apps/lan-ui` are untouched. **There is no LAN import
  endpoint**, and there must never be one: import is the Host's, and a
  participant must not be able to reach it.
- The Host UI gains one meeting-level panel. No bulk import, no import history,
  no roster redesign.
- **A hand-edited file naming a different valid participant of the same meeting,
  bearing a `submission_id` never imported before, will still import into that
  participant's note if the Host confirms without reading the preview.** The
  cross-participant check catches a reused artefact, not a freshly forged one.
  This is inherent to a model where a person is the authority, and it is why the
  preview shows the database's own participant name and why import is never one
  click.
- A documentation reconciliation item remains open and is recorded rather than
  silently resolved: PRD section 12 line 421 and section 12.2 line 429, and
  architecture rules section 12 lines 342-344 and 351, describe a Host who
  chooses *reject / replace / new version* for a possible duplicate, and describe
  a same-id-different-hash file as a correction. Under decisions 6 and 7 the Host
  has no such choice: every duplicate, modified and cross-participant artefact is
  refused, and a correction is a newly generated form. The PRD and the
  architecture rules are **not edited here**; this paragraph is the record.
