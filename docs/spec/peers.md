# Peers

A peer is a device, identified by a signing key. What a peer may do is decided per [group](groups.md), by the role the group gives it.

## Identity

- A peer's key is its identity everywhere: in transport, in signatures, and in [membership](membership.md).
- A key belongs to one device and is never shared or moved.
- A person with several devices is several peers; grouping them is what a group is for.
- A removed key is never readmitted. A device that returns does so under a new key.

## Roles

- A contributor may write to the group and counts toward its [quorum](membership.md).
- A witness counts toward the quorum and never writes. It vouches for the order of [checkpoints](checkpoints.md), not for their contents.
- An observer holds no role in the configuration. It reads, verifies, and relays.
- A peer's role is whatever the latest checkpoint it holds says; nothing else grants one.

## Observers

- Any number of peers may observe a group; the group does not know or track them.
- An observer verifies everything a contributor would, so data relayed by an observer is as good as data from its author.
- An observer chooses how much history to keep. One that keeps all of it is an archive; see [compaction](compaction.md).
- TODO: nothing limits who may observe. Confidentiality needs encryption that survives relaying and removal, and is not designed.

## Trust

- An observer or relay is trusted with nothing: it cannot forge, reorder, or withhold undetectably what it does not hold a key for.
- A contributor is trusted not to equivocate and not to sign a checkpoint it has not checked. Breaking either leaves proof; see [log](log.md).
- Every contributor may write anything in the group. There is no authority narrower than the group.
- TODO: permissions narrower than a group are expressed today only by splitting data across groups.
