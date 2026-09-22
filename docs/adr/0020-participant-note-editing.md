# 0020. Participant note editing over the LAN

- Status: Accepted
- Date: 2026-09-22

Implements PRD sections 5.2 and 15. Depends on ADR-0002 and ADR-0016 for the
session model, ADR-0012 for the mutation boundary, ADR-0014 for the split read
path, ADR-0018 for the realtime channel, and ADR-0019 for the Markdown subset
and the editor.

## Context

Step 8 gave the Host a note editor. PRD section 15 also gives a participant the
right to create and edit **their own** note, and PRD section 5.2 is explicit
about the other half: a participant may not see anybody else's.

Almost everything needed already existed. `authorize()` has permitted
`Actor::Participant` to write their own note since step 2; `app-core::note` has
validated content for whichever transport calls it since step 8; `note.changed`
has reached the note's owner, and only the owner, since step 7. What was
missing was a route, a read model, a body limit and a screen.

That shaped this decision: the smallest change that closes the gap, and no
redesign of anything that already works.

## Decision 1: the route carries no identifiers

```text
GET /api/note      the participant's own note, or null
PUT /api/note      replace it
```

Not `/api/meetings/{meeting_id}/me/note`, and not `/api/note/{participant_id}`.

The meeting and the participant are already established by the authenticated
session, so the route accepts neither. The difference matters: a path parameter
would have to be *checked* against the actor on every handler that ever touches
it, and a check can be forgotten. A parameter that does not exist cannot be.

`/api/session` already works this way and says so in its own documentation —
"there is nothing to pass that would select a different participant". These
routes follow it rather than introducing a second convention.

The request body is one field, `content`. No `participant_id`, no `meeting_id`,
no `note_id`, no `expected_version`. A field that is absent cannot be trusted by
mistake, which is a stronger guarantee than a field that is read and ignored.

## Decision 2: identity comes from the session, as it already did

The existing `Participant` extractor is reused unchanged:

```text
Authorization: Bearer <session token>
  -> hash_token
  -> SessionStore::resolve_session   (revoked_at IS NULL, in the SQL)
  -> Actor::Participant { meeting_id, participant_id, session_id }
```

Every field comes from the row. The `participant_id` the handler passes to
`WriteNote` is a **target**, and `authorize()` refuses it unless it equals the
actor's own — so the domain, not the transport, is the final authority. A
revoked session resolves to nothing and is answered 401, identically to a token
that never existed.

**No `app-core` change was required.** Not one line.

## Decision 3: a read model of the participant's own

`ParticipantQueries::own_note(meeting_id, participant_id)` returns `content`,
`version`, `updated_at` and `last_author_type`.

`HostQueries::note` is deliberately **not** reused. ADR-0014 names read models
for their audience precisely so that the convenient struct from one audience
does not end up serving another; the Host's shape carries a note id, a creation
timestamp and an author id that a participant has no use for.

The participant DTO omits, in order of how tempting each was to include:

| Omitted | Why |
| --- | --- |
| `note_id` | the participant addresses the note implicitly through their session; an identifier they cannot use to address anything is one more thing in a browser. `SessionView` omits the session id for the same reason |
| `last_author_id` | for one's own note the only possible authors are self, Host or import — the type alone is unambiguous |
| meeting and participant ids | the client already has the session view |
| lock state | already carried by `GET /api/session`; two sources could disagree |

`last_author_type` stays, because it answers a question the participant
genuinely has: whether the Host changed their note underneath them.

## Decision 4: two body limits, doing two jobs

The global LAN body limit stays at **1 KiB** for every other route and for the
asset fallback. `/api/note` gets a **route-scoped** override of **192 KiB**,
attached to the route rather than the router.

| Limit | Job |
| --- | --- |
| 1 KiB, global | nobody makes the Host allocate for an identifier |
| 192 KiB, one route | nobody makes the Host allocate for a note |
| 64 KiB, `app-core::note` | the actual rule about how long a note may be |

192 KiB is three times the content limit: enough for JSON escaping of a body
made entirely of quotes or backslashes, far short of unbounded. The domain
remains the size authority, so a body between 64 and 192 KiB is read and then
refused with a message naming the limit and the measured size, while a body past
192 KiB is refused by the transport with a 413.

The extractor ordering is load-bearing and is asserted by a test: `Participant`
runs before the body is touched, so the larger limit never lets an
**unauthenticated** caller make the Host allocate 192 KiB.

## Decision 5: `payload_too_large`, a new refusal

HTTP 413, kind `payload_too_large`, category `validation`.

Distinct from a validation refusal on purpose. A participant who sent 300 KiB
and one who sent 70 KiB have different problems, and only the second can be told
the exact limit. Collapsing them would have made the common case worse to
diagnose.

The pre-existing malformed-JSON behaviour of `routes::claim` is **not** changed;
it is a separate inconsistency and not this step's to fix.

## Decision 6: reading survives a lock, writing does not

`GET /api/note` works while the meeting is `LOCKED`. A locked meeting is
finished, not secret, and the participant wrote this note.

`PUT /api/note` is refused, by `ensure_mutable()` re-read **inside** the
mutating transaction. The participant UI hides the controls, and that is
presentation: a stale screen that calls the API anyway gets the same 409.

Under the current claim flow a participant session can only exist for an `OPEN`
meeting — `claim_identity` calls `ensure_open()` and nothing returns a meeting
to `DRAFT` — so `ensure_mutable()` is both sufficient and consistent with PRD
section 15. No lifecycle change was needed.

## Decision 7: last-write-wins, with the draft protected

No `expected_version`, no optimistic concurrency, no merge. The version is
derived inside the transaction and `UNIQUE(note_id, version)` decides a race, as
it has since step 1.

What the UI adds is protection for unsaved text. When `note.changed` arrives
while the editor is open, the draft is left exactly as it is and the participant
is offered *Keep editing* or *Discard mine and reload*. Saving anyway is
permitted; what it replaces is still in the note's history.

A version the bundle already knows is its own save echoing back — possibly to a
second tab — and is ignored. That is display logic over a value the server
returned, not concurrency control: no precondition is ever sent.

## Decision 8: the realtime channel is untouched

`note.changed` already carried the note id and the version and never the
Markdown, and its audience has always been `Participant(meeting, participant)`.
A participant therefore cannot receive another participant's note event at all —
the audience stops it at the server, so there is nothing for the bundle to
filter or accidentally render.

No new event, no new audience, no inbound frame, no change to the socket.

## Decision 9: one editor, reused

`packages/editor` is used unchanged. The participant bundle wraps
`renderMarkdown` in the same ten-line ref component the Host uses, because the
renderer is framework-neutral by decision (ADR-0009) and returns DOM nodes
rather than markup. There is no second parser, no second validator and no HTML
sink on either side.

## Consequences

- Participants can write their own notes, and the MVP flow in PRD section 24
  (*join, select identity, write note, host sees note*) is complete end to end.
- `app-core` and the migrations are untouched. Step 8B is a transport, a read
  model and a screen.
- The LAN error contract gains one kind. The TypeScript union is exhaustive, so
  both sides know about it.
- Five boundary guards replace the one that asserted this step had not happened:
  the route addresses nobody, the response carries no identifiers, the
  participant read path stays its own, the body limit is route-scoped, and the
  participant bundle has no history view and sends no identity.
- **Note links remain deferred.** PRD section 5.2 grants a participant up to
  five links, and that is still unbuilt: `note_versions` versions content only,
  and what a version means for links is the open question ADR-0019 recorded. The
  requirement stands; only the implementation is outstanding.
- A note now travels the LAN in cleartext alongside the bearer token that
  already did. The join URL and the transport were already documented as an
  operational secret on a plain-HTTP LAN (ADR-0002); this widens what is on the
  wire without changing that boundary, and it is worth stating rather than
  discovering.
