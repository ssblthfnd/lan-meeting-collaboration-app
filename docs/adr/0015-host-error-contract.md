# 0015. The Host error contract

- Status: Accepted
- Date: 2026-09-21

Settles the question ADR-0012 deliberately left open: "`DomainError` is the
public contract for transports. It carries no SQLite types and no HTTP status
codes; mapping onto a transport's vocabulary is that transport's job."

## Context

`DomainError` is a Rust enum with ten variants. A Tauri command returning
`Result<T, E>` rejects with whatever `E` serializes to, so the shape of that
serialization *is* the contract the Host UI programs against, whether or not
anyone designs it.

The default is to let it happen by accident. `E = String` is the path of least
resistance: `error.to_string()` reaches the UI, and the UI then either shows raw
text for every failure or starts matching on substrings. Both are bad in the same
way - the error becomes untyped at exactly the boundary where the architecture
asks for the most care. Architecture rules section 22 requires errors to be
actionable and to name the expected and the detected value; a UI that cannot tell
a validation refusal from a disk failure cannot present either one well.

Two further constraints:

- `DomainError::Conflict` and `DomainError::Persistence` carry diagnostic text
  that originates in SQLite - "UNIQUE constraint failed: note_versions.version",
  "attempt to write a readonly database". `error.rs` already says that text "is
  not part of the contract and must not be parsed". Serializing it would publish
  it to the UI anyway, where it would be shown to a person who cannot act on it
  and, being visible, would eventually be matched on.
- Whatever shape is chosen here will be copied by the LAN transport. An HTTP
  status code baked in now would be wrong for Tauri and would then look like
  precedent.

## Decision

Every Host command returns `Result<T, HostError>`, where `HostError` serializes
to one flat JSON object with three parts:

```json
{
  "kind": "meeting_not_draft",
  "category": "lifecycle",
  "message": "meeting 0199… is no longer being prepared: expected status DRAFT, detected OPEN",
  "meeting_id": "0199…",
  "detected": "OPEN"
}
```

### `kind` - the precise refusal

One value per `DomainError` variant, `snake_case`, with that variant's own fields
alongside it. Serde's internally-tagged representation, flattened, so the
TypeScript side is a discriminated union and narrowing on `kind` yields exactly
the fields that kind carries.

### `category` - the coarse grouping

Six values: `authorization`, `not_found`, `lifecycle`, `validation`, `conflict`,
`unexpected`. This is what decides *how* a failure is presented - an inline field
message, a refusal notice, a reload prompt - and it lets a view handle every
failure without enumerating ten kinds. A view that wants to be specific still has
`kind`.

Two levels rather than one because the two questions are genuinely different.
"Which input was wrong" needs `kind` and `field`; "should this be red or amber,
and is retrying worth offering" needs only the category.

### `message` - the domain's own sentence

Taken from `DomainError`'s `Display`, not composed in the mapping layer. The
domain already phrases these to name the expected and the detected value, and
maintaining a second wording in the transport would mean the clearer of the two
is the one nobody sees. The UI shows this string.

### Diagnostics stay on the device

`Conflict` and `Persistence` serialize with **no fields**. Their detail is written
to stderr on the Host machine and replaced with a sentence the person can act on
("Another change was saved first. Reload and try again."). The UI is told what
happened, not how SQLite phrased it. Nothing is sent anywhere - stderr is the
Host's own console, and no data leaves the device (PRD section 24).

### Parse failures are ordinary validation

A malformed identifier, date, time or timezone is refused by the transport before
the domain sees it, and is reported as `kind: "validation"` with the same
`field` / `expected` / `detected` fields a domain refusal carries. The UI
therefore needs one error path, not a separate one for "the frontend sent
something that was not even a UUID".

This is the boundary establishing types, not the boundary making decisions.
Whether a title is acceptable, whether a schedule is coherent and whether the
roster has room are decided in `app-core` inside the transaction - the transport
passes those through untouched, and a test asserts that a blank title parses
successfully and is refused by the domain.

### No HTTP status codes

This transport has no HTTP. A status code here would be meaningless and would
then be copied into the LAN transport as though it had been designed. A test
asserts no `status`-like key appears in the serialized form.

## Consequences

- The UI branches on data. `ErrorNotice` renders any failure, and a form can
  highlight the input named by `field` without knowing which rule rejected it.
- Adding a `DomainError` variant forces a decision here: the mapping is an
  exhaustive `match`, so it will not compile until the new variant is given a
  kind and a category.
- The UI never sees a constraint string or a Rust `Debug` output. A test asserts
  both directions - that the diagnostic is absent and that a useful message is
  present.
- The categories are a contract of their own. Renaming one is a breaking change
  to the frontend, which is why there are six broad ones rather than one per
  kind.
- The LAN transport will map the same `DomainError` to its own shape, where HTTP
  status codes *do* belong. It should reuse the `kind`/`category` vocabulary and
  must not reuse the types: an error body for a participant is a different
  audience (ADR-0014).
