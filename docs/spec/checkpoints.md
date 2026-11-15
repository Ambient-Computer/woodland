# Checkpoints

A checkpoint is a quorum's signed statement that a stretch of history is final. Checkpoints form one chain per [group](groups.md), and that chain is the only source of finality.

## Contents

- A checkpoint names the checkpoint before it, so the chain is a sequence and not merely a set.
- It carries the [configuration](membership.md) in force after it.
- It carries a frontier: for each [clock](groups.md), how far along each contributor's [chain](log.md) it covers.
- A frontier is causally closed, and never behind the previous checkpoint's.
- For each clock it commits to the history root of each chain at the frontier, and to a root of the clock's state after everything covered.
- It is valid when signed by a quorum of the configuration it extends.

## Agreement

- No two valid checkpoints ever name the same predecessor. Agreeing on the next checkpoint is consensus, and is treated as such.
- Safety holds whoever is offline, and however messages are delayed, dropped, reordered, or relayed.
- Progress needs a quorum connected, directly or through relays, long enough to exchange a few messages.
- Everything said during agreement is signed and relayable, so the signers need not meet.
- A signer persists what it promises before saying it. A signer that forgets a promise can break agreement.
- A [contributor](peers.md) signs only after checking the frontier and recomputing the roots for the clocks it holds. A witness checks only that the chain is extended.
- TODO: the agreement protocol is not chosen. It must be leaderless in the sense that any quorum can finish what any member started.

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
