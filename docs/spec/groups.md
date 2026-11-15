# Groups

A group is a set of [contributors](peers.md) and everything they agree on: who they are, and one history of what they wrote. It is the unit of membership, finality, and trust.

## Shape

- A group has one [configuration](membership.md) and one [checkpoint](checkpoints.md) chain.
- A group owns any number of clocks.
- A clock owns any number of [models](models.md).
- A group is named by its founding: who founded it, and something no other founding shares.
- Nothing signed for one group is valid in another.

## Clocks

- A clock is one causal history: the unit a peer subscribes to, [syncs](sync.md), and [compacts](compaction.md).
- A [transaction](transactions.md) belongs to exactly one clock and may change any of that clock's models.
- Clocks in a group are causally independent. Nothing in one waits on, or is ordered against, anything in another until a checkpoint covers both.
- A peer that subscribes to a clock holds all of it; a peer that does not holds none of it.
- Data that different devices need in different amounts belongs in different clocks.
- Creating a clock is writing to it; it needs no agreement. Which contributors may write to it is a matter of [role](peers.md).

## Composition

- An application may use many groups, with different contributors.
- Groups do not transact with each other. A peer that contributes to one and observes another reads the second and writes what it concludes to the first.
- Account-like data is a small group other groups' members observe.
- An aggregate view over many writers is a peer that observes their groups and contributes to its own.
- A large audience is served the same way: a peer reads a [sealed](confidentiality.md) group and republishes what the audience may see.
- A peer's roles in different groups are unrelated. Trust never crosses a group boundary.
