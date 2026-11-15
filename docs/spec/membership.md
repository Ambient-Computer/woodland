# Membership

A configuration says which [peers](peers.md) hold a role in a [group](groups.md) and how much each counts. It changes only through the [checkpoint](checkpoints.md) chain.

## Configuration

- A configuration lists the group's contributors and witnesses, and a weight for each.
- A quorum is any set of them holding more than half the total weight.
- Any two quorums of one configuration share a member. Every safety property of finality rests on that overlap.
- Weights are the application's choice. One always-on device may outweigh the rest, so that it and any one other form a quorum.
- A configuration where one peer holds more than half the weight makes that peer an arbiter. One where every peer is needed makes finality unanimous. Neither is a special case.
- A founder alone is a valid configuration, and commits by itself.

## Changing it

- A change is proposed by a contributor and takes effect when a checkpoint carries it.
- The checkpoint that carries a change is signed by a quorum of the configuration it replaces.
- Checkpoints after it are signed by a quorum of the new configuration.
- The chain of configuration changes, from the founding, is the proof of who the members are. A peer that trusts the founding can verify any later configuration.

## Joining

- A peer joins by being told the group's founding and a recent checkpoint by a peer it trusts, then observing.
- That checkpoint is the joiner's anchor. A removed contributor's key can still sign a plausible alternative past, and the anchor is what rules it out.
- A new contributor writes once it holds the checkpoint that admits it, and builds on that checkpoint.
- Adding an observer needs nothing from the group.

## Removal

- Removing a contributor needs a quorum that does not include it. Its consent and its presence are not required.
- The checkpoint that removes a contributor is the cut. What it covers of the contributor's chains counts; nothing else of them ever will.
- A removed contributor cannot place work before the cut, whatever time or position it claims.
- A lost or stolen device is removed the same way as a retired one.
- Removal is how a group stops waiting on a contributor that will not return.

## Liveness

- Writing never needs a quorum. Changing membership and committing do.
- A group below quorum keeps working optimistically and commits nothing until a quorum reconnects.
- TODO: a group that permanently loses more than half its weight can never commit or reconfigure again. Recovery needs a decided escape, and weights should be chosen with that in mind until there is one.
