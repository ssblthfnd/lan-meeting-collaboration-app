# 0012. The domain mutation boundary

- Status: Accepted
- Date: 2026-09-21

## Context

The architecture states that both transports must route every mutation through
`app-core` (CLAUDE.md, architecture rules section 3), and that the meeting lock
must be re-read *inside* the mutating transaction because a pre-flight check is
a race rather than an enforcement (section 15).

Those two requirements pull against each other. `app-core` owns the rules and
must not own SQL, but a rule that has to be evaluated inside a transaction
cannot be evaluated without reaching the database. Three transports are coming -
the LAN HTTP server, Tauri commands, and the remote-import pipeline - and if
each opens its own transaction and remembers to check the lock, then "remembers"
is the whole safety argument. The first transport that forgets is a silent
correctness bug, and it will be invisible in review because the missing code is
the absence of a line.

A second problem: the persistence layer is a crate like any other. Nothing
stops `app-server` from depending on `app-db` directly and writing a row.

## Decision

### A port, owned by the domain

`app-core` defines what it needs from persistence and `app-db` implements it:

- `port::DomainTx` - the reads and writes available within one transaction.
- `port::Database` - `transaction(&self, f)`, which runs `f` inside a single
  `BEGIN IMMEDIATE` transaction.

`app-core` gains no SQLite dependency, and `app-db` holds mechanics only: no
authorization decision, no lock check, no business rule. The dependency arrow
still points from `app-db` to `app-core`, unchanged.

`Database::transaction` takes `&mut dyn FnMut(&dyn DomainTx)` rather than a
generic closure, keeping the trait object-safe at the cost of the service
capturing its result in a local. A generic-associated-type formulation was
considered and rejected: it forces every future transport to name higher-ranked
lifetimes for no benefit the domain can use.

### One mutation path

`service::Domain` is the only mutation entry point. Every operation runs the
same pipeline, in this order, inside one transaction:

```text
load current meeting state -> authorize -> check the lock -> validate
  -> write entity -> append note history -> append audit -> COMMIT
```

The meeting is loaded from the database on every mutation. Nothing consults a
`Meeting` the caller supplied, a cached value or an earlier check. Because the
pipeline exists once, a transport cannot skip a step by forgetting it - there is
no per-transport copy to forget.

Broadcasting stays outside and after the commit (section 16).

### Authorization as an unforgeable proof

`authz::authorize` returns an `Authorized`. Every **write** method on
`DomainTx` requires one, and `Authorized`'s fields are private to `app-core`.

This is the part that matters. The adapter that *implements* the write methods
cannot construct an `Authorized`, and neither can a transport that depends on
`app-db`. Writing to the database outside the boundary is not a lapse of
discipline that review has to catch; it is code that does not compile.

`Authorized` also carries the actor discriminator and the acting participant
id, both derived from the `Actor` rather than from a payload. Audit records and
`note_versions.created_by_type` take their values from it, so an actor cannot be
misattributed even by a buggy caller. Where a write needs a meeting id - setting
a status, inserting a note - the adapter reads it from the proof, not from the
payload.

### What is not decided here

Session and token resolution stay in the transport layer: `app-core` receives an
`Actor` that has already been established (architecture rules section 14.1). The
domain never parses an identity out of a request body.

## Consequences

- The lock rule is enforced in exactly one place for all transports, present and
  future. A mutation racing a lock loses deterministically, proven by tests that
  run real threads against one SQLite database rather than mocking the lock.
- Note version numbers are derived from current state inside the transaction,
  and `UNIQUE(note_id, version)` (ADR-0011) turns a lost race into a
  `Conflict` instead of duplicate history.
- A refusal and a failure both roll the transaction back, so "a failed mutation
  leaves no audit record" holds by construction rather than by discipline.
- `app-core` is testable without a database for the rules that do not need one,
  and the rules that do need one are tested against real transactions in
  `app-db`, because that is where both halves are available.
- The cost is indirection: adding a domain operation means adding port methods
  as well as the service method. That is deliberate - the port is the list of
  things persistence is allowed to do, and it should be uncomfortable to grow.
- `DomainError` is the public contract for transports. It carries no SQLite
  types and no HTTP status codes; mapping onto a transport's vocabulary is that
  transport's job.
