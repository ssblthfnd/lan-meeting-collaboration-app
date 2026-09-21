# 0016. The LAN session and claim model

- Status: Accepted
- Date: 2026-09-21

Implements ADR-0002 (first-claim-wins) and settles five questions it left open.
Depends on ADR-0012 for the mutation boundary and ADR-0014 for the read path.

## Context

ADR-0002 decided *that* a LAN participant binds to one identity and that the
Host can approve, reject or revoke. Building it surfaced five questions that had
to be answered together, because the answers constrain each other.

## Decision 1: approval is acknowledgement, not a gate

PRD 5.2.1 gives the Host approve and reject and lists `PENDING` as a state they
see. ADR-0002 rejected "the Host approves every join" because it is a bottleneck
at the start of a meeting, when everyone joins at once. Read together, the only
reading that satisfies both is:

- A claim creates a session that is **live immediately**. `approved_at` stays
  null.
- The participant may do everything a participant may do, from that moment.
- **Approve** sets `approved_at`. It grants nothing.
- **Reject and revoke** are the same act: they set `revoked_at`, and a revoked
  session cannot act.

So `PENDING` means *joined and working, not yet ticked off by the Host* - not
*waiting for permission*. That reading is load-bearing in the UI too: the Host's
roster labels it "Joined", the participant's screen says "you do not need to wait
for that", and no code path anywhere reads `approved_at` to decide anything. A
test asserts the two sessions differ only in a flag.

The consequence worth stating: a wrongly-taken identity is **detected and
corrected**, not prevented. The Host sees the claim and revokes it. That is the
trade ADR-0002 already made when it rejected per-participant codes, and this
decision does not reopen it.

## Decision 2: `Actor::Claimant`

A claim happens *before* a session exists, so the actor performing it cannot be
`Actor::Participant` - that type carries a `session_id`, and inventing one inside
the very type whose job is to say "this request was authenticated" is how a
type stops meaning anything.

A fourth variant, `Actor::Claimant { meeting_id, participant_id }`:

- authorized for **exactly one** operation, `ClaimIdentity`, and only for the
  identity it names;
- refused for everything else, including writing a note - so "holding a join URL
  does not let you write" is a compile-time fact rather than a review item;
- `actor_type()` returns `"PARTICIPANT"`, because a claim *is* performed by a
  participant.

That last point is what avoids a migration. A fourth `actor_type` value would
mean altering the `CHECK` constraints on both `audit_logs.actor_type` and
`note_versions.created_by_type`, and splitting a vocabulary that ADR-0008 and
ADR-0011 both depend on, to say something the existing word already says.

`participant_id` on a claimant is a **target**: it names which identity is being
requested. Authority comes from possessing the join token, and who wins a race is
decided by the database. This is the same distinction `WriteNote.participant_id`
already carries (architecture rules 14.1 rule 5).

## Decision 3: tokens, and where they may exist

Two credentials, both 256 bits from the OS CSPRNG, both stored **only** as a
SHA-256 hash (PRD 22.2, 22.17).

| | Join token | Session token |
| --- | --- | --- |
| Travels in | the URL path | `Authorization: Bearer` |
| Stored in | `meetings.join_token_hash` | `participant_sessions.session_token_hash` |
| Proves | you were given the link | you already claimed an identity |
| Establishes | `Actor::Claimant` | `Actor::Participant` |

The enforcement is a type. `app_core::token::TokenHash` parses only 64 lowercase
hexadecimal characters, `app-core` has no `sha2` and no `rand`, and every write
method takes a `TokenHash`. A plaintext token in any other encoding will not
parse, so the persistence layer cannot be handed a secret by mistake. The
transport mints and hashes; the domain stores the result and never sees the
input.

`subtle` was listed in ADR-0006 for constant-time comparison and is deliberately
**not** added: every lookup is an indexed equality match on a hash column, so
nothing in Rust compares a secret. ADR-0006's table is a commitment about which
crate to reach for if one is needed, not an obligation to use all of them.

### Issuing, rotating, and what closes a join URL

Issuing is an explicit Host action on an `OPEN` meeting. It is **not** automatic
on opening: minting a credential is a decision.

Re-issuing overwrites the stored hash, so the previous token matches nothing and
the old URL resolves to no meeting. That is the whole of rotation - there is no
revocation list because there is only ever one live token.

**Locking a meeting does not clear the hash.** The join URL stops working because
every request re-reads the meeting's status, which is the check that has to hold
anyway (architecture rules 15). Clearing the column would be a second mechanism
that could disagree with the first.

## Decision 4: the server is started by the Host

Opening a meeting does not start the LAN server. Binding a LAN-reachable socket
raises the Windows Firewall prompt and makes the machine answer on the network,
and that should happen because someone chose it.

"Tied to the meeting lifecycle" is honoured at the **request** level instead,
which is the stronger guarantee: one server, one port, and which meetings are
joinable is decided per request by the join token and the meeting's current
status. A meeting that locks mid-session stops accepting participants
immediately, with no restart and no cached state.

The Host also chooses which local address the join URL advertises. A laptop can
be on Wi-Fi, Ethernet and a VPN at once and the server answers on all of them,
but the URL can only name one, and only the person in the room knows which
network the participants are on. Loopback is offered and clearly marked, because
`127.0.0.1` must never be assumed reachable by a participant (architecture
rules 4).

## Decision 5: what a participant may see, and when

Two tiers:

| Holding | May see |
| --- | --- |
| a join token | the meeting's public facts; **names and whether they are taken** |
| a session token | the above, plus **their own** full details |

A name is unavoidable: a participant picks their own identity from the list
(PRD 5.2). Department, position and role are not needed for that, so they are
withheld until the claim succeeds.

Enforced by **shapes, not filters**. `ParticipantQueries` and the LAN DTOs have
no field for another participant's details, a note, an audit entry, a token hash,
a session id or a participant count. Leaking one would require adding a field, a
column and breaking a test - not forgetting a `WHERE`.

The participant error contract follows the same rule and carries **no
identifiers at all**: an unknown join token and an unknown route return the same
404, so a refusal cannot be used to discover whether an id is real.

## Consequences

- A participant is fully able to take part the moment they claim, which is what
  makes a meeting startable without the Host clicking ninety-nine times.
- `PENDING` must never be presented as a blocked state. It is the normal state.
- A join URL is an operational secret and anyone holding it can take an
  *unclaimed* identity. Unchanged from ADR-0002; the remedy is Host revocation,
  which is why revoke is in this step and not deferred.
- The identity list exposes every participant's **name** to anyone holding the
  join URL. Inherent to letting people pick their own name; stated here so it is
  a known property rather than a surprise.
- LAN transport remains plain HTTP, so tokens cross the network in the clear.
  That boundary was stated in ADR-0002 and is not re-opened here.
- Adding a real approval gate later would contradict Decision 1 and needs a
  superseding ADR, not a new branch in the session resolver.
