# Checkpoints

A checkpoint is a quorum's signed statement that a stretch of history is final. Checkpoints form one chain per [group](groups.md), and that chain is the only source of finality.

## Contents

- A checkpoint names the checkpoint before it, so the chain is a sequence and not merely a set.
- It carries the [configuration](membership.md) in force after it.
- It carries a frontier: for each [clock](groups.md), how far along each contributor's [chain](log.md) it covers.
- A frontier is causally closed, and never behind the previous checkpoint's.
- It names every clock that has committed state, whether or not it covers anything new there. Any one checkpoint is then enough to check a [snapshot](compaction.md) of any clock.
- For each clock it commits to the history root of each chain at the frontier, and to a root of the clock's state after everything covered.
- It is valid when signed by a quorum of the configuration its predecessor carries.

## Agreement

- No two valid checkpoints ever name the same predecessor. Agreeing on the next checkpoint is consensus, and is treated as such.
- Each position in the chain is decided once, on its own, by the configuration its predecessor carries.
- An attempt to fill a position has a rank no other attempt shares. Anyone in the configuration may start one, at any time, outranking those it has seen.
- A signer that sees an attempt promises it: it will not vote in any lower-ranked attempt, and it reports the last checkpoint it voted for.
- An attempt with promises from a quorum names a checkpoint. If any promise reported a vote, it names the checkpoint of the highest-ranked vote reported, and not its own.
- A signer votes for the checkpoint named by the highest-ranked attempt it has not promised against.
- Votes from a quorum, for one checkpoint in one attempt, are the checkpoint's signatures.
- A checkpoint that could have gathered a quorum's votes is therefore the only one any later attempt can carry. That is the whole of the safety argument.

## Statements

- Everything said during agreement is a signed statement bound to one position in one chain. It means the same whoever relays it and however late.
- There are no requests and no replies. A peer keeps the latest statement of each kind from each signer, passes them on, and acts on what it holds.
- Signers therefore need not meet. A quorum connected through relays, at different times, decides.
- Any signer can finish an attempt another started, by outranking it and inheriting what it gathered.
- A signer persists a promise or a vote before anyone can hear it. A signer that forgets one can break agreement; see [storage](storage.md).
- Two statements from one signer that a correct signer could not both make are proof against it, like a [fork](log.md).

## Validation

- Promising requires nothing but the chain. Voting requires checking what is voted for.
- A [contributor](peers.md) votes only once it holds everything inside the frontier, has checked the frontier is closed, and has recomputed the roots for itself.
- A contributor that cannot yet check a checkpoint syncs until it can. Its vote waits; its promise does not.
- A witness votes on the chain's shape alone: the predecessor, the configuration, and that the frontier does not go back.
- An attempt may have to name a checkpoint its own author has not checked, because a promise reported it. Naming is not vouching; the author still votes only after checking.
- A checkpoint that fails a contributor's check is never voted for by it. Contributors holding enough weight to block a quorum can therefore stop a bad checkpoint, and a quorum of careless ones cannot be stopped.

## Progress

- Safety holds whoever is offline, and however statements are delayed, dropped, repeated, reordered, or relayed.
- Progress needs a quorum connected, directly or through relays, long enough for a few statements to cross.
- Rival attempts that keep outranking each other decide nothing. A contributor whose attempt is outranked waits before trying again, and longer each time.
- A contributor does not start an attempt while one it has promised is still making progress.

## Ordering

- A checkpoint's transactions are those inside its frontier and outside its predecessor's.
- The committed [timeline](log.md) is each checkpoint's transactions in chain order, and within one checkpoint in uncommitted timeline order.
- Everything not covered sorts after everything covered, whatever logical time it carries.
- Order is a pure function of the checkpoint chain and the covered transactions, so every peer derives the same one.

## What finality means

- A committed transaction's place and [verdict](transactions.md) never change.
- Committed state is the same on every peer that holds the same checkpoint, and only moves forward.
- Two peers at different checkpoints never disagree; one is behind.

## Late work

- A contributor that was away when a checkpoint was made keeps what it wrote. Its transactions take their place after the checkpoint.
- Their guards are judged at that new place. Work that conflicts with what was committed is pruned; the rest stands.
- Being offline never loses an unconditional write. It can lose a race.
- Committed work beats uncommitted work regardless of when either was written. This is the cost of not waiting for everyone.

## Cadence

- A checkpoint is worth making whenever a quorum is connected and something uncommitted exists.
- Nothing requires checkpoints at any rate. A group that makes none still works, with only [optimistic reads](reads.md) advancing.
