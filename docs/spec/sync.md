# Sync

Sync is what two connected [peers](peers.md) exchange so each ends up with everything the other has that it wants. It is the same exchange whatever roles the two hold.

## Interest

- Each side says which [groups](groups.md) and [clocks](groups.md) it wants. Only the intersection is exchanged.
- Interest is per connection and may change during it.
- What a peer wants of one connection may be less than it holds. That narrows the transactions it is sent there, not a [snapshot](compaction.md), which is the state of everything it holds or of nothing.
- A peer need not want everything it holds, or hold everything it wants.
- A peer with a say in [checkpoints](checkpoints.md) wants every clock: a vote vouches for all of them.
- A peer that leaves a clock out still follows the chain, and commits checkpoints without that clock's state.
- Having committed a checkpoint without a clock, a peer cannot start wanting it. It lets go of the group and joins again.
- Nothing but the chain is sent to a peer before it has said what it holds and wants.
- A peer offers a [snapshot](compaction.md) only to one that wants no more than it holds.

## Exchange

- For each shared group, each side says which [checkpoint](checkpoints.md) it holds. The side behind is sent the chain it lacks, and verifies it link by link.
- For each shared clock, each side says how far along each [chain](log.md) it holds, and is sent what it lacks.
- A side that lacks transactions the other has [compacted](compaction.md) is sent a snapshot, then what is uncommitted.
- Checkpoint agreement, fork proofs, and membership proposals travel the same way: each side sends what the other has not seen.
- After catching up, the connection stays open and carries new data as it happens. Catch-up and live are one protocol.

## Properties

- Sync is idempotent and resumable. A dropped connection loses nothing and repeats little.
- Sends are best-effort. Nothing depends on a message arriving, only on peers connecting again.
- Work is bounded per exchange. No step needs either side to hold a whole clock in memory.
- A peer never advertises what it has not made durable.
- A peer's account of what it holds is taken as it stands. It may hold less than it did, having let go of what a [removal](membership.md) or a [fork](log.md) made void.
- A peer may turn down what it is sent and want it later. What was sent is offered again after a while, not at once.
- Cost is proportional to the difference between the two sides, not to the size of what they share.

## Validation

- Everything received is verified before it is stored: signatures, links, membership, and whatever roots the receiver is able to check.
- Nothing is trusted because of who delivered it.
- Invalid data is dropped and the rest of the exchange continues. A peer that keeps sending it may be disconnected; that is the application's policy.

## Relaying

- Whatever a peer holds, it can serve. Data crosses any number of hops unchanged and equally verifiable.
- Two contributors that never connect directly converge if a path of connections links them over time.
- A relay need not understand a clock's [models](models.md) to carry it, nor be able to [read](confidentiality.md) it.
- A group's liveness depends on connectivity over time, never on a particular topology.

## Transport

- Sync assumes an ordered, reliable, authenticated stream between two keys, and nothing else.
- Discovery, addressing, and relays are the transport's concern.
