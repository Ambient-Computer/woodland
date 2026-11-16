# Commit Operations

A commit operation is one whose effect depends on state its author cannot know, so it is evaluated once, when a [checkpoint](checkpoints.md) covers it. It is how the database does what no device can decide alone.

## Why they exist

- A [guard](transactions.md) names keys. "Every row where", "no row refers to this", and "the next number" name whatever exists at a place in the [timeline](log.md).
- Evaluating such a thing optimistically would give each peer a different answer, revised with every arrival.
- Evaluating it at commit gives one answer, on a prefix that will never change.

## Behaviour

- A commit operation is written in a transaction like any other, and is carried, signed, and ordered the same way.
- Its effect appears in neither [read](reads.md) level until its transaction commits. An application can see that it is pending.
- It is evaluated at its transaction's place in the committed timeline, against the state every earlier committed transaction left.
- If it cannot apply, its transaction is pruned, finally.
- The guards of its own transaction are judged without its effect, like every other guard.
- Its effects are part of the checkpoint's state, so the checkpoint's signers have agreed on what it did.

## The set

- Update or delete every row matching a condition.
- Delete a row together with everything that refers to it, or only if nothing does.
- Assign the next value of a dense sequence.
- Enforce a rule over several rows or cells from here on.
- Apply a [migration](migrations.md) that removes or constrains.
- The set is fixed by the library and small. An application composes these; it does not add to them.

## Determinism

- A commit operation is evaluated by every peer, so it is pure: no time, no randomness, no local state.
- Its meaning never changes once released. A changed meaning is a new operation.
- Evaluation is identical on every target, bit for bit. Anything that could differ, float formatting or collation, is defined by the library and not the platform.

## Cost

- A commit operation is as slow as the group's next checkpoint, which may be days away.
- An application offers them where a wait is acceptable, and uses guarded transactions where it is not.
