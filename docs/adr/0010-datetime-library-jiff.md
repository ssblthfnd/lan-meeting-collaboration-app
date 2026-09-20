# 0010. Date-time library: jiff

- Status: Accepted
- Date: 2026-09-20

Resolves the open choice in ADR-0006, which listed `jiff` **or**
`chrono` + `chrono-tz` and deferred the decision to Phase 1, step 1. ADR-0006
otherwise stands.

## Context

ADR-0005 made time explicit: system timestamps are UTC RFC 3339, a meeting
stores an IANA `timezone`, and `date` / `start_time` / `end_time` are read in
that timezone. It also required tests for UTC-to-meeting-timezone conversion
and for DST, deliberately using a zone the target region never exercises.

That produces three distinct kinds of time value, and the whole class of bug
ADR-0005 exists to prevent is one of them being silently treated as another:

| Kind | Example |
| --- | --- |
| An instant, absolute, always UTC | `created_at`, `locked_at` |
| A zoneless calendar date and time | `date`, `start_time`, `end_time` |
| The named zone that gives the second kind meaning | `meetings.timezone` |

Both candidate libraries can express all three. The question is which makes the
distinction hard to lose.

A check of the existing build was worth doing before arguing from convenience:
`cargo tree -i jiff` and `-i chrono` both print nothing for
`x86_64-pc-windows-msvc`. Both crates appear in `Cargo.lock` for *other*
targets only, so neither is already compiled here and neither starts with a
"it's free, it's already there" advantage.

## Decision

Use **`jiff`** (with the `serde` feature). Do not add `chrono` or `chrono-tz`.

Reasons, in the order they mattered:

1. **The type system draws the same three lines the schema does.** `Timestamp`
   is an instant, `civil::Date` and `civil::Time` are explicitly zoneless,
   `TimeZone` is a named IANA zone, and `Zoned` is the combination. Mixing them
   up is a compile error. The chrono equivalent assembles `NaiveDate`,
   `NaiveTime`, `DateTime<Utc>` and a `chrono_tz::Tz` from two crates, where
   "naive" is a naming convention rather than a barrier.

2. **DST is total rather than optional.** `to_ambiguous_zoned` returns a value
   that names the gap or the fold, so a meeting scheduled into a DST gap can be
   reported to the Host as a data-entry problem. chrono's `LocalResult` models
   the same thing but is routinely collapsed with `.unwrap()`, and the failure
   is invisible until a real transition.

3. **The tzdb is bundled.** Windows ships no system timezone database, and the
   Host must work with no internet at all. `jiff` embeds `jiff-tzdb` on
   platforms that lack one. `chrono-tz` compiles the database in too, so this
   is parity on capability — but it is one dependency rather than two.

4. **Fixed-precision RFC 3339 output.** `%.3f` prints exactly three fractional
   digits, which is what makes the stored timestamp fixed width. See ADR-0011:
   that property is what keeps `ORDER BY created_at` chronological.

5. **One dependency, one API, one mental model**, aligned with the TC39
   Temporal design that other languages are converging on.

## Consequences

- `app-core` gains a `jiff` dependency. `chrono` and `chrono-tz` are not added,
  and should not be: two date-time libraries in one workspace is how the
  distinction between naive and aware values gets lost again.
- Storage formats are defined once, in `app_core::time`, and enforced a second
  time by `CHECK` constraints in the migration (ADR-0011).
- A local time that does not exist, or exists twice, is an error the Host sees
  rather than a value the code silently picks. Callers must handle
  `TimeError::DstBoundary`.
- The DST tests use `America/New_York` deliberately, as ADR-0005 anticipated:
  Indonesian zones have fixed offsets and would never reach that code path.
- If a future requirement genuinely needs chrono (for example an external crate
  that only speaks `chrono::DateTime`), that is a superseding ADR, not an extra
  line in `Cargo.toml`.
