# Reads

A read is local and never waits on another peer. The reader chooses how certain the answer must be.

## Levels

- An optimistic read sees every live [transaction](transactions.md) the peer holds. It is the latest the peer knows, and may be revised.
- A committed read sees only what a [checkpoint](checkpoints.md) covers. It is the same on every peer at that checkpoint, and never goes back.
- The level is chosen per read, not per database or per connection.
- A model may restrict itself to committed reads, for values others act on; see [models](models.md).

## Mixing levels

- A guard built from an optimistic read is as provisional as the read.
- A decision that must be final is made from committed reads, or waits for its own transaction to commit.
- The two levels are distinct to the application, so one cannot be mistaken for the other.

## Queries

- Reads are computed over local state, so what can be asked is limited by the query engine and not by replication.
- A query that is not monotone, a count or an absence, is stable only at the committed level.
- Derived structures, [indexes](tables.md) and aggregates, are maintained locally and incrementally, by retracting and asserting as verdicts change.
- The cost of a late arrival is proportional to what it changes, not to how much history follows it.

## Watching

- A peer reports what changed, at which level, so a query can be re-run or updated.
- A report says where to look, not what is there. It may be reported when nothing a reader sees differs.
- A committed read changes only when a checkpoint arrives. An optimistic read changes when a transaction arrives, is pruned, or is revived.
