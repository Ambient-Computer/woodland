# Log

The log is every [transaction](transactions.md) a peer holds, as signed chains. It gives each transaction an author, a place in that author's history, and a place relative to everyone else's.

## Chains

- Each [contributor](peers.md) has one chain per [clock](groups.md), and appends to it alone.
- An entry is a signed header and a payload. The header commits to the payload, the entry before it, and earlier entries at growing distances.
- A header is verifiable without its payload, and without the entries between it and any earlier one it can reach.
- A header names the [checkpoint](checkpoints.md) its author held when writing it.
- A chain has a history root: a commitment to all of it that grows by appending and proves any entry's inclusion.

## Causality

- An entry names the heads of the other chains its author had integrated in that clock.
- A peer integrates an entry only after everything it names. A peer's history is always one its authors could have seen.
- Two entries that do not reach each other are concurrent.
- Names are hashes, never counters. A counter cannot tell two entries at the same position apart.

## Timeline

- The timeline is one total order over a clock's transactions, the same on every peer that holds the same set.
- Transactions covered by a checkpoint come first, in the order the [checkpoint chain](checkpoints.md) fixes.
- The rest follow, ordered by a logical time that respects causality, with ties broken by author.
- A transaction never sorts before one it was written on top of.
- The uncommitted part of the timeline changes as transactions arrive. The committed part never does.

## Forks

- Two entries signed by one contributor that name the same predecessor are a fork.
- The two headers are a fork proof: self-contained, checkable by anyone, and valid forever.
- Fork proofs are kept and relayed like any other signed data, by every peer that sees one.
- A contributor with a fork proof against it is removed by the next checkpoint; see [membership](membership.md).
- Of a forked chain, what counts is what a checkpoint covered. Nothing else of it is integrated.
- The goal is accountability, not tolerance. A group of a few devices cannot outvote a liar, but it can prove one.

## Validation

- An entry is accepted only if its signature and links hold, and the checkpoint it names lets its author [write that clock](peers.md).
- An entry that fails is dropped, whoever sent it; a relay is never blamed for its author.
- Validation never depends on which peer delivered an entry, or in what order.
