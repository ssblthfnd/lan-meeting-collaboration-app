# 0001. Record architecture decisions

- Status: Accepted
- Date: 2026-09-20

## Context

The PRD and the architecture rules describe *what* the system does and *which*
rules hold. They do not record *why* a particular option was chosen over the
alternatives, and that reasoning is what gets lost first. Several decisions in
this project are deliberate trade-offs that will look like oversights to a
future reader: plain HTTP on the LAN, no PDF in Phase 1, one note per
participant.

## Decision

Significant architectural and dependency decisions are recorded as ADRs in
`docs/adr/`, numbered sequentially.

A decision that is later changed is not edited in place: a new ADR supersedes
the old one, and the old one records which ADR replaced it.

The PRD and the architecture rules document remain authoritative for
requirements and rules; ADRs carry the reasoning and the alternatives
considered.

## Consequences

- Every non-obvious choice has a findable rationale.
- Reviewers can challenge a decision by writing a superseding ADR rather than
  by arguing in a commit message.
- One extra file per real decision. Routine implementation choices do not need
  an ADR.
