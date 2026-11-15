# Rationale

The spec states decisions without their history. This page keeps the history: what was built, what was read, and what was turned down. Papers were surveyed through summaries in November 2026 and mostly not read in full.

## What the first prototype taught

- It ordered transactions by logical time and made them final when every contributor had acknowledged them, using Reconfigurable Lattice Agreement to settle who the contributors were.
- Recording effects and guards, with verdicts replayed over one order, worked. It gave tables, uniqueness, compare-and-set, and references from three kinds of guard, and it is kept.
- Signing everything and treating every protocol as "merge what you hear" worked. Devices that never met converged through ones that could not write, and it is kept.
- Finality by unanimity did not work. One closed laptop stopped commits, compaction, and deletes for everyone, and the only cure was removing it.
- Membership changes were hard for the same reason: each had to be acknowledged by everyone before anything else could settle.
- Removing a contributor had no clean moment. Deciding which of its writes counted needed vouching, and peers that had taken the wrong ones had to rewind.
- Storing indexes as logged writes made indexed columns exclude concurrent edits. An index is derivable, and is now [derived](../spec/tables.md).
- Requesting storage through the state machine made every read a round trip, and added a third verdict for "waiting on storage". Storage is now [awaited](../spec/library.md).

## Why a chain and not a lattice

- Lattice agreement decides values that contain one another. It cannot say which of two concurrent things came first.
- Guards need exactly that. So the prototype could only finalize what nobody could still write before, which is unanimity.
- A chain of checkpoints is a sequence, so "everything outside sorts after" is well defined. Late work is then reordered rather than waited for.
- A sequence costs consensus. Consensus cannot guarantee progress without timing assumptions, but lattice agreement already needed a connected quorum, so nothing was given up in practice.
- Deciding one position at a time, by the configuration the previous position named, removes the hardest part of reconfiguration: there is never more than one configuration to satisfy.

## Sources of finality

- The literature surveyed offers three: a quorum that overlaps, acknowledgement by everyone, or a designated arbiter. Nothing found provides finality with none of them.
- Weighted quorums make all three the same mechanism with different weights, which is why the spec has one rule.
- Systems that settle membership without any of them converge but never finalize, and all of them found leave a removed member able to backdate.
- The closest recent proposal sorts events by arbiter-issued epochs before anything else. The checkpoint ordering rule is the same idea with a quorum as arbiter.

## Why not Byzantine fault tolerance

- Tolerating one liar needs four members and three of them online. Groups here have three members and one online.
- Signed, hash-linked statements make lying leave evidence instead. Evidence plus removal is what a small group can actually use.
- This is a deliberate limit: a dishonest quorum can commit anything.

## Why effects and not statements

- Replaying statements means every device runs the same application code, bit for bit, forever, including code from versions it never had.
- Replaying effects means devices agree only on the library. Impure work happens once, on the author's device.
- The cost is that an effect cannot say "every row where". Those cases are [commit operations](../spec/commit-operations.md), evaluated once on final history.
- The split follows a known result: an invariant can be kept without coordination only if concurrent valid changes always merge into a valid state. The rest must wait for agreement, and here that means waiting for a checkpoint.

## Why storage is awaited

- Three shapes were considered: storage as requests and replies through the state machine, storage called synchronously, and storage awaited.
- Requests were the prototype's shape. They kept the core pure and made every read a continuation written by hand.
- Synchronous calls fixed that and were briefly the decision. In a browser they leave one option, a file in a dedicated worker, which drags the whole application behind a message port.
- Awaiting gets the same code as synchronous calls, with the suspension written by the compiler. It works over a browser database, a blocking native store, and memory alike.
- The price is that a step can be abandoned halfway, and that code on the read path is marked as waiting. Both are contained: an abandoned core reloads, and only storage is ever waited on.
- Determinism survives because a step runs alone and waits on nothing but its store.
- Owning the drivers, rather than accepting any store an application brings, was the project author's suggestion. It removes storage callbacks from every binding and lets one test suite vouch for every store.

## Why confidentiality is simple here

- Most local-first encryption work exists to handle membership changes that happen concurrently. A checkpoint chain has none.
- So a secret per checkpoint, delivered in the checkpoint, is sufficient; the elaborate constructions are not needed.
- Forward secrecy is given up on purpose. A new member must be able to read existing data, and the two are incompatible.

## Reading list

- Reconfigurable Lattice Agreement: Kuznetsov, Rieutord, Tucci-Piergiovanni, 2019.
- Coordination Avoidance in Database Systems: Bailis et al., 2015. The source of the invariant result above.
- Byzantine Eventual Consistency: Kleppmann and Howard, 2020. What can be had with no coordination at all.
- Bayou: Terry et al., 1995. Tentative and committed writes, with a primary where Woodland has a quorum.
- UniStore: Bravo et al., 2021. Mixing optimistic and strong operations in one history.
- ERA, Epoch-Resolved Arbitration: Dougal, 2026. Epoch-first ordering against backdating.
- Self-Healing Lattice Agreement: de Souza et al., 2021, and PeerReview: Haeberlen et al., 2007. Accountability in place of tolerance.
- Bamboo: Meyer. Append-only logs verifiable in part.
- DBSP: Budiu et al., 2023. Incremental views by retraction.
- Cambria: Litt et al., 2021. Schema change with old writers.
- Keyhive: Ink & Switch. The concurrent-membership problem this design avoids.

## Open questions

- Whether the checkpoint cost on a large state stays bounded per step, and what the state commitment must look like for that.
- How a contributor decides when to start a checkpoint attempt without wasting others' time.
- What a merging model needs from the core beyond a pure function, and whether text fits.
- How much of confidentiality survives contact with real readers at scale.
