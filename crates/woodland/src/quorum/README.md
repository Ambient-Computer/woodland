# The Quorum

A leaderless variant of multi-paxos that exists to answer "who can write to this data?"

## Why

CRDTs are multi-writer and optimistic. They're famously bad at garbage collection. Integrating events requires preserving the log, a log that's deleted under compaction.

Compaction is only safe if you can guarantee no device has writes before some point in history, i.e., everyone is at or above some point in history.

The quorum fills this gap without requiring a central coordinator.

## Behavior

A quorum initializes with a set of allowed contributors. They are the only actors allowed to vote toward the next quorum state.

The quorum only manages membership. Application state belongs to CRDTs.

Votes are asynchronous and replicated as signed payloads. Commits are eventually recognized by each contributor during sync.

All transactions order by epoch. Commits produce a new epoch, meaning any transaction after recognizing consensus is ordered later than transactions at the old epoch.

## Checkpointing and Compaction

Each contributor maintains a matrix clock. It carries the last-known epoch and transaction version of every other contributor. Signed clocks propagate during sync.

Checkpoints are the greatest lower bound of clock state. Once every contributor recognizes a transaction (and all its predecessors), it becomes safe to compact. That point in history is immutable.
