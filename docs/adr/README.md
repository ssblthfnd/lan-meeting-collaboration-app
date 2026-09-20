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

Template:

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by ADR-XXXX
- Date: YYYY-MM-DD

## Context
## Decision
## Consequences
```
