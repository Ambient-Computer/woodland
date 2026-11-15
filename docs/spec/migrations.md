# Migrations

A migration changes a schema while devices that have not heard of it keep writing against the old one. Schema changes are operations in the [timeline](log.md), so they merge, order, and commit like data.

## Additive changes

- Adding a table, a column, or an [index](tables.md) is an ordinary [transaction](transactions.md) and takes effect optimistically.
- Additive changes commute with each other and with every write to what already existed.
- A device that has not seen an addition is unaffected by it. Its writes remain valid.

## Restrictive changes

- Dropping a table or column, or adding a rule existing or concurrent data might break, is a [commit operation](commit-operations.md).
- It takes effect at one place in the committed timeline. Before it the old schema held; after it the new one does, on every peer.
- Uncommitted work that falls after it and names what it removed is pruned.
- A new rule that committed data already violates fails, and its transaction is pruned. Repairing the data first is the author's job.

## Old writers

- A transaction is kept as its author wrote it, against the schema its author had. It is never rewritten.
- A rename is an alias: writes under the old name remain valid and land in the same place.
- A peer always understands every schema operation in history it holds. Schema is data, not application code.
- An application release must tolerate rows written before its migrations and by devices that have not upgraded.

## Application versions

- The schema a group has agreed to act on is a committed value. A device reads it to learn what it may assume.
- A device older than the committed schema keeps syncing and relaying. Whether it keeps writing is the application's choice.
- Library versions are a separate matter: a peer that meets an operation it does not know stops integrating that clock, and does not guess.
