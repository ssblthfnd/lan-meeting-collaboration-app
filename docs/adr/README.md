# Architecture Decision Records

One file per decision, numbered, append-only in spirit: a decision that changes
gets a new ADR that supersedes the old one rather than an edit that erases the
history.

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-lan-identity-first-claim-wins.md) | LAN identity binding: first-claim-wins | Accepted |
| [0003](0003-single-note-per-participant.md) | Exactly one note per participant per meeting | Accepted |
| [0004](0004-defer-pdf-export.md) | Defer PDF export to Phase 2 | Accepted |
| [0005](0005-explicit-timezone-handling.md) | Explicit timezone handling | Accepted |
| [0006](0006-initial-dependency-set.md) | Minimal initial dependency set | Accepted |
| [0007](0007-note-canonical-format.md) | Canonical note format: GFM-subset Markdown | Accepted |
| [0008](0008-phase1-schema-refinements.md) | Phase 1 schema refinements | Accepted |
| [0009](0009-remote-form-no-framework.md) | Remote form uses no UI framework | Accepted |
| [0010](0010-datetime-library-jiff.md) | Date-time library: jiff | Accepted |
| [0011](0011-identifier-and-storage-formats.md) | Identifier type and canonical storage formats | Accepted |
| [0012](0012-domain-mutation-boundary.md) | The domain mutation boundary | Accepted |
| [0013](0013-draft-only-configuration-and-roster.md) | A meeting's configuration and roster are settled in DRAFT | Accepted |
| [0014](0014-host-read-path.md) | Reads do not go through the mutation boundary | Accepted |
| [0015](0015-host-error-contract.md) | The Host error contract | Accepted |
| [0016](0016-lan-session-and-claim-model.md) | The LAN session and claim model | Accepted |
| [0017](0017-local-qr-generation.md) | Local QR generation | Accepted |
| [0018](0018-realtime-events-and-presence.md) | Realtime events, audiences and presence | Accepted |
| [0019](0019-note-editing-and-markdown-subset.md) | Note editing, the Markdown subset and safe rendering | Accepted |
| [0020](0020-participant-note-editing.md) | Participant note editing over the LAN | Accepted |
| [0021](0021-remote-form-generation.md) | Remote form generation and the submission artifact | Accepted |
| [0022](0022-remote-submission-import.md) | Remote submission import | Accepted |
| [0023](0023-meeting-export.md) | Meeting export: lifecycle, audit and the Rust renderer | Accepted |

Template:

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by ADR-XXXX
- Date: YYYY-MM-DD

## Context
## Decision
## Consequences
```
