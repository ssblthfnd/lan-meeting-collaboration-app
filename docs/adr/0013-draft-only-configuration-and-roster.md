# 0013. A meeting's configuration and roster are settled in DRAFT

- Status: Accepted
- Date: 2026-09-21

Builds on ADR-0012, which established the single mutation boundary. This ADR
decides *when* two particular kinds of mutation are allowed, not how they are
routed.

## Context

The lifecycle is `DRAFT -> OPEN -> LOCKED` (PRD section 6). What each status
permits is specified for notes and for locking, but not for the meeting's own
configuration or its participant roster, and the two documents that mention it
do not agree:

- PRD section 5.1 says the Host may "edit the meeting **before lock**", which
  reads as: configuration stays editable through `OPEN`.
- PRD section 19 lists what `LOCKED` forbids, including "participants cannot be
  added" and "meeting metadata cannot be changed" - implying both were possible
  up to that point.
- PRD section 7, on the other hand, describes configuration and the participant
  list as what the Host decides *when creating* the meeting, and section 8.1
  describes `OPEN` as the point at which a join URL and QR code are published to
  participants.

Left unresolved, each mutation would decide for itself, and the answer would be
whatever the first implementation of each happened to do.

The question matters because `OPEN` is the moment the meeting becomes visible to
other people. Once a participant has scanned a QR code, claimed an identity and
started writing, changing the schedule underneath them, renaming them, or
deleting the identity their session is bound to are not configuration edits -
they are changes to something other people are already acting on. Removing a
participant in particular cascades to their note, their note history and their
session (ADR-0011), so "the Host tidied the roster" and "a participant's work
vanished mid-meeting" would be the same operation.

Three options were considered:

1. **Configuration and roster mutable until `LOCKED`.** Closest to a literal
   reading of PRD section 5.1, and the most permissive. It makes every reader of
   a meeting - the Host UI, a participant's browser, a generated remote form -
   a cache that can silently go stale, and it gives destructive roster edits a
   window in which they destroy live data.
2. **Mutable only in `DRAFT`.** What a meeting *is*, and who is in it, are
   decided while the Host is still preparing it. After that the meeting is a
   fixed frame that notes are written into.
3. **Mutable in `OPEN` but only for non-destructive changes** - say, editing a
   title but not removing a participant. Defensible, but it replaces one clear
   rule with a per-field table, and the PRD offers no basis for drawing the line
   in any particular place.

## Decision

Option 2. A meeting's configuration and its participant roster are mutable
**only while the meeting is `DRAFT`**.

- `Domain::create_meeting` creates a meeting in `DRAFT`. There is no way to ask
  for another status, so `OPEN` is always reached through
  `Domain::open_meeting` and is therefore always audited.
- `Domain::update_meeting`, `add_participant`, `update_participant` and
  `remove_participant` each re-read the meeting's status inside their own
  transaction and refuse unless it is `DRAFT`.
- `Meeting::ensure_draft` is the single definition of that rule, as
  `ensure_mutable` is for the lock. The two are deliberately separate checks
  rather than one stricter version of the other: notes are the opposite case and
  stay writable through `OPEN`.
- The refusal is a new error, `DomainError::MeetingNotDraft`, carrying the
  expected and the detected status. It is not `Forbidden` - no actor may do
  this, so it is not a question of the actor's role - and it is not
  `MeetingLocked`, which is the stronger statement that nothing may change at
  all. A `LOCKED` meeting still refuses with `MeetingLocked`, because the lock is
  checked first and is the more informative fact.
- There is no separate "unlock" or "reopen for editing". Adding an edit-while-
  `OPEN` flow later is a superseding ADR, not an extra branch.

### The 99-participant limit

PRD section 7 caps a meeting at 99 participants. The limit is enforced in two
places, for the reason ADR-0011 gives for the five-link cap: a rule that lives
only in Rust is a rule some future code path can skip.

- `Domain::add_participant` counts the roster **inside** the mutating
  transaction and refuses the addition that would exceed it, with an error naming
  the limit and the count that was found.
- Migration `V2` adds a trigger that refuses a hundredth row outright. A write
  that reached the database another way fails as a constraint violation, which
  the existing mapping turns into `DomainError::Conflict`.

Because the count is read inside a `BEGIN IMMEDIATE` transaction, two concurrent
additions cannot both see room for the same place. This is tested with real
threads against one database, not by mocking the count.

The matching minimum of one participant is deliberately **not** enforced yet. A
meeting under construction necessarily passes through an empty roster, so the
only meaningful place to check it is the moment a meeting opens - and that is a
rule about opening, which belongs to the step that decides what else opening
requires.

## Consequences

- A participant's view of the meeting cannot go stale in a way that matters: the
  frame is fixed before anyone can see it. The same holds for a generated remote
  form, whose immutable metadata (architecture rules section 7) is a copy of the
  configuration - it cannot be contradicted by a later edit.
- A destructive roster edit cannot reach live participant data. `OPEN` is
  precisely the point after which notes and sessions can exist.
- The Host's workflow gains a real constraint: a correction discovered after
  opening cannot be made. This is the accepted cost, and it is the narrower
  reading of PRD section 5.1 rather than a contradiction of PRD section 19, which
  describes what `LOCKED` forbids without promising that `OPEN` permits it.
- A late arrival cannot be added to an open meeting. They can still take part as
  a remote participant only if they were on the roster when it closed, so in
  practice the Host prepares the roster generously before opening. If this proves
  too strict in use, the fix is a decision - a superseding ADR - not a quiet
  relaxation of the check.
- Participant names remain deliberately non-unique within a meeting. Neither the
  PRD nor the schema makes them unique, two people genuinely can share a name,
  and identity is the participant id: a name arriving in a request or a
  submission file is data, never authority (architecture rules sections 10 and
  14.1).
