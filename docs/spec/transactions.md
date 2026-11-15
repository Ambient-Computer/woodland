# Transactions

A transaction is an atomic group of operations by one [contributor](peers.md), and the unit that is signed, sent, ordered, and judged. It records what its author did and what that depended on, never how to redo it.

## Effects

- The author reads local state, decides, and writes down the result: values placed under keys, and keys cleared.
- No peer re-runs the author's logic. Anything impure, a random ID or the time, is evaluated once by the author and recorded.
- A transaction may change any [model](models.md) in its [clock](groups.md).
- A recorded time is the author's claim and orders nothing.

## Guards

- A guard is something the transaction requires of one key at its place in the [timeline](log.md).
- There are three: the key is vacant, the key is present, the key holds exactly what the author read.
- Vacant is how exactly one transaction gets something. Present is how one transaction depends on another. Holds is how a write depends on a read.
- A guard names keys known when the transaction is written. A condition over keys not yet known is a [commit operation](commit-operations.md).
- A write with no guard is unconditional: of two to one key, the later in the timeline is what is read.

## Verdicts

- A transaction whose guards all hold is live. One with a guard that fails is pruned.
- A pruned transaction stays in the log, since others were written on top of it, and none of its operations take effect in any model.
- A verdict depends only on the timeline before the transaction, so every peer with the same history reaches the same one.
- A verdict is provisional until the transaction is [committed](checkpoints.md), and final after.
- A transaction arriving late can flip the verdicts of those after it, in either direction.
- Verdicts are decided without interpreting values. What a model declares its operations write and require is all that is consulted.

## Isolation

- A transaction is atomic: every peer sees all of it or none of it.
- A peer reads its own writes immediately.
- Optimistic work is causally consistent and not serializable. Two transactions conflict only on keys one of them guarded.
- An author that needs a read to still be true guards it.
- A transaction built on one that is later pruned is pruned with it only if it guarded what that one wrote.

## Consequences for applications

- Success is provisional. A transaction the author saw succeed may later be pruned, and the application is told.
- An application that must not act on provisional success waits for the transaction to commit.
- Rejection is all or nothing and never silent; there is no partial merge of a guarded transaction.
