# Membership

A configuration says which [peers](peers.md) hold a role in a [group](groups.md) and how much each counts. It changes only through the [checkpoint](checkpoints.md) chain.

## Configuration

- A configuration lists the group's contributors, witnesses, and readers, with a weight for each contributor and witness.
- A quorum is any set of them holding more than half the total weight.
- Any two quorums of one configuration share a member. Every safety property of finality rests on that overlap.
- Weights are the application's choice. A device that is reliably reachable and reliably durable deserves more than one that is neither.
- A configuration where one peer holds more than half the weight makes that peer an arbiter. One where every peer is needed makes finality unanimous. Neither is a special case.
- A founder alone is a valid configuration, and commits by itself.
- A contributor's weight may be zero. It writes, and has no say in what is committed or in who the members are.
- A configuration with no weight, or with no contributor, is never valid: nothing could follow it. Nobody votes for a checkpoint that carries one.

## Changing it

- A change is proposed by a member that holds weight, and takes effect when a checkpoint carries it.
- A checkpoint that changes the configuration is voted for only in an attempt started by a member that holds weight. A writer confined to a [clock](peers.md) cannot vote itself out of it.
- The checkpoint that carries a change is signed by a quorum of the configuration it replaces.
- Checkpoints after it are signed by a quorum of the new configuration.
- Two configurations are never in force at once. Each decides exactly one checkpoint: the next.
- The chain of configuration changes, from the founding, is the proof of who the members are. A peer that trusts the founding can verify any later configuration.

## Joining

- A peer joins by being told the group's founding and a recent checkpoint by a peer it trusts, then observing.
- That checkpoint is the joiner's anchor. A removed contributor's key can still sign a plausible alternative past, and the anchor is what rules it out.
- The anchor comes with the checkpoints that changed the configuration on the way to it, so the joiner verifies it from the founding and not on the inviter's word alone.
- What the inviter is trusted for is that the anchor is on the group's real chain. A joiner believes whoever invited it.
- A joiner does not replay history up to its anchor. It takes the committed state there as a [snapshot](compaction.md), from any peer, and checks it against the anchor.
- Until it has that state, a joiner accepts no entry from an author the anchor's configuration leaves out.
- A new contributor writes once it holds the checkpoint that admits it, and builds on that checkpoint.
- Adding an observer needs nothing from the group.

## Removal

- Removing a contributor needs a quorum that does not include it. Its consent and its presence are not required.
- The checkpoint that removes a contributor is the cut. What it covers of the contributor's chains counts; nothing else of them ever will.
- A removed contributor cannot place work before the cut, whatever time or position it claims.
- An entry names the latest [checkpoint](checkpoints.md) its author held. If any checkpoint from that one on leaves the author out, the entry is void, wherever and whenever it is received.
- The cut takes what was written before the removing checkpoint. Work written under a later one, by a contributor since added back, survives a peer that folds the removal late.
- A contributor that is added back continues its chains from the cut. Positions past the cut are reused, and a void entry at one is not a [fork](log.md).
- A lost or stolen device is removed the same way as a retired one.
- Removal is how a group stops waiting on a contributor that will not return.
- A contributor proven to have [forked](log.md) or to have broken a [promise](checkpoints.md) is removed by the next checkpoint any honest contributor proposes.

## Liveness

- Writing never needs a quorum. Changing membership and committing do.
- A group below quorum keeps working optimistically and commits nothing until a quorum reconnects.
- A group survives losing a member only if the rest still hold more than half the weight. Weights are chosen so that losing any one device leaves a quorum.
- One peer holding half the weight or more is a single point of failure for finality, however convenient it makes commits.

## Succession

- A group that has permanently lost its quorum can never commit or reconfigure again. No rule inside the group can change that without breaking the overlap safety rests on.
- Its survivors found a successor: a new group whose founding names a checkpoint of the old one and starts from that checkpoint's committed state.
- Succession is a decision by people, not a protocol step. Nothing can prove a quorum is gone for good rather than away.
- A peer follows a successor the way it joins any group: by being told, by someone it trusts.
- Two sets of survivors may each found a successor. They are different groups from then on, and nothing reconciles them.
- Uncommitted work in the old group is not carried over. Its authors write it again in the successor.
- The old group is not ended by succession. A quorum that returns finds its chain intact and its peers elsewhere.
