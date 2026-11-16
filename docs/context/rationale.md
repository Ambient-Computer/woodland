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

## Why the core has no query engine

- Considered in November 2026: growing the read surface toward joins, aggregates, and a planner, or leaning on an existing engine for them.
- Reads are local and never voted on. A wrong plan costs time and not agreement, so an existing engine would be safe there, which it would not be on the write path.
- The engines looked at were DataFusion, GlueSQL, and SQLite kept as a derived copy.
- DataFusion is the most capable and the heaviest. A community build for the browser was about 30 MB, by the project author's reading of it; no published figure was found. That rules it out of a library shipped to browsers.
- GlueSQL is built around storage supplied by its host and runs in a browser. It is much lighter and much less proven. Read about, not tried.
- SQLite as a derived copy is the most proven engine there is, and a second store to keep in step with the first. It has no good home in a browser, and optimistic state fits it badly.
- None of them answers the common case in a browser, which is keeping something on screen current. Their unit is a query run once.
- So the core stops at what only it can do, bounded ordered reads and reports of change, and everything richer is built from those by the application.
- The same two things serve every engine an application might want: all of committed state at a known checkpoint, and what each later checkpoint changed. That is the [mirror](../spec/reads.md).
- Bindings to particular engines were rejected even as optional features of the core. A feature's dependencies are still built and tested with the core, on every target.
- Incremental view maintenance is the relational answer to live results, and DBSP is in the reading list for it. At the sizes expected here, reading again is cheaper than maintaining a view, and nothing prevents an application from doing either.
- A mirror that falls behind reads everything again. Keeping reports for slow consumers would be unbounded growth inside the core, and a consumer that minds can keep its own.

## Why nothing general runs on the write path

- What a filter matches and what a rule allows are worked out on every device and must come out the same on all of them, across every version there will ever be.
- A general engine there would make its planner, its arithmetic, and its text handling part of the protocol. The vocabulary is kept small so that all of it can be frozen.
- A cell that is added to, a counter, was considered and dropped. Next to being set it needs a rule for which wins, and next to a bound such as stock never below zero it is wrong by construction.
- Both cases already have answers that are right: allowances for a bounded quantity, and a merging model for an unbounded total.

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

## Why the state root is a sum

- A checkpoint commits to the state it produces, and every voter recomputes it. Whatever does that runs on every device at every checkpoint.
- A tree over the state proves any part of it, and costs a walk through storage for every changed key. Keeping one balanced inside an ordered key-value store is a project of its own.
- A sum of hashes costs one addition per changed key and nothing to store but itself. It answers the first open question below: a checkpoint's cost follows what changed.
- A sum of short hashes is forgeable, since a set matching a chosen total is far easier to find than a collision. The construction used has rows long enough that it is not; it was read about, not analysed here.
- The price is that a snapshot cannot be checked piece by piece, and a reader cannot be shown one row with a proof. Neither is needed yet.
- If either becomes needed, the sum can be split into many sums by key, which proves a part at the cost of a larger commitment. That is a format change, and is cheaper to make before anything is deployed.

## Reading list

- Reconfigurable Lattice Agreement: Kuznetsov, Rieutord, Tucci-Piergiovanni, 2019.
- Coordination Avoidance in Database Systems: Bailis et al., 2015. The source of the invariant result above.
- Byzantine Eventual Consistency: Kleppmann and Howard, 2020. What can be had with no coordination at all.
- Bayou: Terry et al., 1995. Tentative and committed writes, with a primary where Woodland has a quorum.
- UniStore: Bravo et al., 2021. Mixing optimistic and strong operations in one history.
- ERA, Epoch-Resolved Arbitration: Dougal, 2026. Epoch-first ordering against backdating.
- Self-Healing Lattice Agreement: de Souza et al., 2021, and PeerReview: Haeberlen et al., 2007. Accountability in place of tolerance.
- Bamboo: Meyer. Append-only logs verifiable in part.
- Securing Update Propagation with Homomorphic Hashing: Lewi et al., 2019. The sum the state root is built on.
- DBSP: Budiu et al., 2023. Incremental views by retraction.
- Cambria: Litt et al., 2021. Schema change with old writers.
- Keyhive: Ink & Switch. The concurrent-membership problem this design avoids.

## Open questions

- Whether proving part of a state will be needed, and what it costs to add once there is data.
- How a contributor decides when to start a checkpoint attempt without wasting others' time.
- What a merging model needs from the core beyond a pure function, and whether text fits.
- How much of confidentiality survives contact with real readers at scale.
- Whether a derived structure an application defines, such as a text index, should live in the core's store and change in the step that changes its source, or stay a [mirror](../spec/reads.md) outside it.
