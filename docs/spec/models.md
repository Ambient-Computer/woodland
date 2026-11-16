# Models

A model is one kind of data inside a [clock](groups.md): a namespace of keys and the operations that write them. [Tables](tables.md) are the primary model; others live beside them in the same history.

## Contract

- A model declares, for each operation, what it writes and what it [requires](transactions.md).
- Everything else follows from the [timeline](log.md) without the model's involvement: who wins, what is pruned, what is final, what is [compacted](compaction.md).
- No layer beneath a model interprets its values.
- A model knows nothing of peers, quorums, or checkpoints.
- Two models never see each other's keys. One transaction may change several.

## Lifetime

- A model comes into existence with the first operation that names it; creating one needs no agreement.
- A model's name includes its clock's. The same name under another clock is another model, and an operation on it belongs in that clock.
- A model's identity includes its kind. An operation of another kind is not valid in it.
- Dropping a model is an operation at a place in the timeline. After it the model is gone for good, and transactions that name it are pruned.

## Built-in kinds

- A table: see [tables](tables.md).
- A map where the last write to a key is what is read.
- A map where the first write to a key holds it: names, locks, anything exactly one writer should get.
- A map of ordered lists, a [merging kind](#merging-kinds): items are inserted, removed, and moved.
- Any map may be restricted to [committed reads](reads.md). Readers then never see a value that could be displaced.

## Merging kinds

- Some data merges rather than replaces: text, ordered lists, trees that move.
- Such a model also declares how concurrent writes to one key combine, as a pure function of those writes.
- That function is held to the same determinism as a [commit operation](commit-operations.md), and its meaning is fixed once released.
- A merging model still commits and compacts by the same rules. Finality lets it discard the metadata merging needed.
- Writes combine in [timeline](log.md) order, so what is read before a write commits is what it commits to if nothing earlier arrives.
- A merge is never [pruned](transactions.md) for what a concurrent merge did. An edit placed beside an item that is gone lands by its other neighbour, or at the end.
- An item moved concurrently is in one place: where the later move put it.

## Extension

- A kind of model is added by the library or by an application built on it, in native code that every peer of the group must have.
- A peer that meets a kind it does not have cannot verify state for that clock. It may still relay it.
- It neither reads nor writes that clock from then on, and [votes](checkpoints.md) for nothing that commits more of it.
- If a quorum commits what it could not read, it takes their word that it is committed. Once it has the kind, it fetches the clock again.
- An operation says which model it is for in a way every peer can read, whatever its kind.
- An operation added to a kind after the kind was released says so the same way. A peer whose library predates it treats it as a kind it does not have.
- An operation that means nothing to the release it names is invalid, to every peer.
- Adding a kind never changes the log, the checkpoint chain, or sync.
