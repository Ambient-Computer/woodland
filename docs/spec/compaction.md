# Compaction

Compaction is how a peer sheds history it no longer needs. It is safe at any time, never changes what the peer would read, and can be undone by any peer that kept what was shed.

## What is kept

- The [checkpoint](checkpoints.md) chain's configuration changes, from the founding. They are the proof of membership.
- The latest checkpoint, with the roots it commits to.
- Committed state as of that checkpoint: one value per key.
- Every [transaction](transactions.md) the checkpoint does not cover.
- Fork proofs against anyone still a member.

## What is shed

- Covered transactions. Their effect is in committed state and their existence is in the chain's history root.
- Superseded values and cleared keys. Nothing can arrive that sorts before a checkpoint, so nothing needs to be remembered to lose to.
- Whatever a [merging model](models.md) kept only to merge with concurrency that can no longer arrive.
- A committed deletion leaves nothing behind.

## Bounds

- A peer's storage is bounded by committed state plus uncommitted history.
- Uncommitted history grows while no quorum is reachable. That is delay, not unsafety, and it ends at the next checkpoint.
- What a peer must load to start is bounded by uncommitted history, not by the size of the data.
- A group with a contributor that will never return is unblocked by [removing](membership.md) it, not by waiting.

## Snapshots

- A snapshot is committed state for one [clock](groups.md), offered to a peer that is behind the sender's checkpoint.
- It is verified against the state root in a checkpoint the receiver has verified. A snapshot is never taken on its sender's word.
- State is committed to in a form that proves a part without the whole, so a snapshot can arrive and be checked in pieces.
- A peer may take a snapshot instead of history even when history is available. The result is the same.

## Restore

- An archive is any peer that kept covered history. Archiving is a local choice and needs no role.
- A peer restores history by asking an archive and checking each entry's inclusion in a history root it retained.
- Restored history is as trustworthy as history never shed. The archive is trusted with nothing.
- History nobody kept is gone. A group that wants its past keeps at least one archive.

## Scope

- Compaction is per clock. A peer may keep all of one clock and the minimum of another.
- Peers compact independently and never coordinate it. Two peers at the same checkpoint with different amounts of history read the same.
