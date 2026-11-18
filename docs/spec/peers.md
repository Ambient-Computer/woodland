# Peers

A peer is a device, identified by a signing key. What a peer may do is decided per [group](groups.md), by the role the group gives it.

## Identity

- A peer's key is its identity everywhere: in transport, in signatures, and in [membership](membership.md).
- A key belongs to one device and is never shared or moved.
- A person with several devices is several peers; grouping them is what a group is for.
- A key that is [removed](membership.md) and later given a role again is the same peer.
- Only a device that lost its [stored state](storage.md) returns under a new key.
- A key and the state it has signed for live and die together; see [storage](storage.md).

## Roles

- A contributor may write to the group and counts toward its [quorum](membership.md).
- A witness counts toward the quorum and never writes. It vouches for the order of [checkpoints](checkpoints.md), not for their contents.
- A reader is named by a [sealed](confidentiality.md) group so that it can read. It never writes and carries no weight.
- An observer holds no role in the configuration. It verifies and relays, and reads whatever it is able to.
- A peer's role is whatever the latest checkpoint it holds says; nothing else grants one.

## Write access

- A contributor writes to every [clock](groups.md) in the group, or only to the clocks its role lists.
- An entry in a clock its author may not write is invalid, like an entry from a stranger.
- Narrowing a contributor's clocks is a removal from those clocks, with the same [cut](membership.md).
- A writer that should not be able to touch other data is given its own clock. This is how an automated writer is confined.
- Nothing is narrower than a clock. Within one, every contributor that may write it may write anything.
- Read access is not divided within a group. Data with different audiences belongs in different groups.

## Observers

- Any number of peers may observe a group; the group does not know or track them.
- A peer that is [removed](membership.md) is an observer from then on, for as long as it stays.
- An observer verifies everything it is able to read, so data relayed by an observer is as good as data from its author.
- An observer chooses how much history to keep. One that keeps all of it is an archive; see [compaction](compaction.md).
- An observer of an open group reads everything. An observer of a sealed group is a relay: it carries what it cannot read.

## Trust

- An observer is trusted with nothing: it cannot forge or reorder, and what it withholds another path can supply.
- A reader is trusted with what it reads, and nothing else.
- A contributor is trusted not to equivocate and not to sign a checkpoint it has not checked. Breaking either leaves proof; see [log](log.md).
- A witness is trusted not to equivocate. It cannot vouch for contents, so a group whose quorum can be met by witnesses alone has no check on contents at all.
