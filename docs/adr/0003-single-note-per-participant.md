# 0003. Exactly one note per participant per meeting

- Status: Accepted
- Date: 2026-09-20

## Context

The PRD was ambiguous. Section 13 read as though a participant could create
several notes, while the remote submission schema in section 11 carries a single
`note` field and the `notes` table has no title or ordering column.

Left unresolved, LAN participants and remote participants would have
structurally different capabilities, and remote import would have no defined
meaning when a participant already had notes.

## Decision

Exactly one note per participant per meeting.

- Database constraint: `UNIQUE(meeting_id, participant_id)` on `notes`.
- A LAN participant creates and edits their own single note.
- The Host may create and edit any participant's note.
- A remote submission represents that participant's single note.
- Remote import is version-aware update/replacement: it updates the existing
  note and appends a `note_versions` row. It never creates a second note.
- History is preserved in `note_versions` and is never deleted.

Multiple notes per participant are explicitly out of scope for MVP.

## Consequences

- Remote import becomes a well-defined upsert, and duplicate handling has a
  single target row to reason about.
- Export ordering is simple: one note per participant, ordered by participant.
- Participants who want to separate topics must do so with headings inside their
  note. Accepted for MVP.
- Moving to multiple notes later is a schema migration plus a submission schema
  version bump, not a small change. This is a deliberate trade for MVP clarity.
