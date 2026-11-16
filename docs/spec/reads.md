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

- A read is a bounded, ordered range: of a [model's](models.md) keys, of a [table's](tables.md) rows by key, or of an [index](tables.md) by value.
- A read takes a limit and says where it stopped, so a range of any size is read a part at a time.
- The core does not join, aggregate, sort, or evaluate expressions. An application computes those from what it read.
- A capability is the core's only if an application could not have it without reading everything. Bounded ranges and indexes are; whatever follows from rows already read is not.
- A query that is not monotone, a count or an absence, is stable only at the committed level.
- Indexes are maintained locally and incrementally, by retracting and asserting as verdicts change.
- The cost of a late arrival is proportional to what it changes, not to how much history follows it.

## Watching

- A peer reports what changed, at which level, so a query can be re-run or updated.
- A report says where to look, not what is there. It may be reported when nothing a reader sees differs.
- A committed read changes only when a checkpoint arrives. An optimistic read changes when a transaction arrives, is pruned, or is revived.
- The expected use is to read, compute, and read again when a report touches what was read. What an application displays is kept current that way.
- The core never reports a query's new answer, or how it differs from the last. A consumer that wants differences reads the keys reported.

## Mirrors

- A mirror is a copy of committed state kept outside the core, in whatever form its owner wants: another database, a search index, a log of events.
- A committed read says which [checkpoint](checkpoints.md) it is of, and a report of committed change says which checkpoint made it. A mirror knows from those where it stands.
- A checkpoint may take a peer several [steps](library.md) to apply. Between them a committed read holds some of the next checkpoint, and says so.
- A mirror is the state of a checkpoint when the report of that checkpoint arrives, and not between.
- A mirror is built by reading everything, and kept by reading what each later checkpoint is reported to have changed.
- Reading everything takes many reads, and a checkpoint may arrive between them. Its report covers whatever the earlier reads missed.
- The core keeps no record of past reports. A mirror that missed one reads everything again, as a peer far behind takes a [snapshot](compaction.md).
- A mirror follows committed state only. Optimistic state is revised, and what a mirror has passed on cannot be.
- An engine that queries a mirror is the application's. The core includes none and binds to none.
