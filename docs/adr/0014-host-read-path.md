# 0014. Reads do not go through the mutation boundary

- Status: Accepted
- Date: 2026-09-21

Completes ADR-0012, which decided how mutations reach the database and said
nothing about reads.

## Context

ADR-0012 established that `service::Domain` is the only mutation entry point,
that every write method on the port requires an unforgeable `Authorized`, and
that the meeting lock is re-read inside the mutating transaction. It is easy to
read all of that as "everything goes through the domain", and the first Host UI
is where that reading has to be either confirmed or rejected: a meeting list, a
roster and an audit trail are all reads, and none of them is a mutation.

Three options:

1. **Reads go through `Domain` too**, as query methods on the same service. One
   entry point for everything, which sounds tidy.
2. **Reads go to a query repository in `app-db`**, beside the write adapter.
3. **A read port in `app-core`**, mirroring `DomainTx`, implemented by `app-db`.

Option 1 is worse than it sounds. `Database::transaction` opens
`BEGIN IMMEDIATE`, which takes the write lock up front - that is deliberate and
is what makes the lock rule enforceable. Routing a list query through it would
make displaying a screen contend for the write lock with actual writes, and
would serialise reads against each other for no reason. It would also put
methods on `Domain` that have no authorization step, no lock check and no audit
record, which makes the one thing `Domain` guarantees harder to see rather than
easier: a reader would have to check, per method, whether it is one of the ones
that enforces anything.

Option 3 buys an abstraction whose only client would be the one implementation
that already exists. ADR-0012 said the port "should be uncomfortable to grow",
and that applies to a second port as well.

The question underneath: **what does the domain decide about a read?** For the
Host, nothing. The Host may see everything in its own database (PRD section 5.1),
there is no lock rule for looking, and a read writes no audit record. A layer
whose rules are all vacuous is not a safeguard; it is a place for a rule to be
added later where nobody expects one.

## Decision

Reads go to `app_db::query`, not through `app_core::service::Domain`.

```text
Mutation:  Host UI -> Tauri command -> Domain -> DomainTx -> SQLite
Query:     Host UI -> Tauri command -> HostQueries       -> SQLite
```

- `HostQueries` holds explicit, typed queries: `meetings`, `meeting`,
  `participants`, `audit_entries`. There is no generic "execute this" method and
  no way to pass SQL in, so the set of readable shapes is a fixed list.
- Results reuse `app-core` types (`MeetingId`, `MeetingDate`, `MeetingStatus`,
  `UtcTimestamp`, `ParticipantDetails`) rather than redeclaring them, so the read
  model cannot drift from the write model.
- Queries run on `Db::read`, the pool of connections opened
  `SQLITE_OPEN_READ_ONLY`. A write attempted through this path fails at the
  SQLite level. The read path is not trusted to behave; it is unable to misbehave.
- **No rules here.** No authorization, no lock check, no filtering that depends
  on meeting status. A caller needing a rule applied asks `app-core`.
- Every query has an explicit **total** order with an id as the final tiebreaker
  (architecture rules section 26.4). Because ids are UUIDv7 that tiebreaker is
  chronological (ADR-0011), so a tie is broken meaningfully rather than
  arbitrarily.

### Named for the audience, not the tables

The type is `HostQueries`, not `Queries`. The Host and a LAN participant are
different audiences: a participant must never receive another participant's note
(architecture rules section 16), and the surest way to leak one is to reuse a
convenient read model that already returns everything. When the LAN transport
arrives it gets its own query type with its own narrower results.

The same reasoning applies to `packages/contracts`, where the Host DTOs live in
`host.ts` rather than in a shared `api.ts`.

## Consequences

- Displaying a screen never contends for the write lock, and reads proceed
  concurrently with a write under WAL. Tested with eight reader threads against
  a writer on one database.
- `Domain` keeps its meaning: every method on it enforces authorization, the
  lock and the lifecycle, and writes an audit record. There are no exceptions to
  check for.
- The cost is that "go through `app-core`" is no longer a single sentence that
  covers every database access. The rule is: **every mutation** goes through
  `app-core`. A read that needs a decision made about it is not a read.
- Two places now know SQL, both inside `app-db`, which is where SQL is supposed
  to be. Neither knows a rule.
- A future requirement that a read *must* be filtered by authorization - a
  participant-facing query - does not bend this decision. It gets its own
  audience-scoped query type whose results are narrow by construction, and if a
  genuine authorization decision is needed to shape one, that decision belongs
  in `app-core` and this ADR is superseded rather than stretched.
