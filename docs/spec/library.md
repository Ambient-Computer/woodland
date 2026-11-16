# Library

Woodland is a library with a deterministic core. The application owns the process, the network, and the clock; the core owns the protocol, the data, and [where the data is kept](storage.md).

## Shape

- The core is a state machine. Its inputs are local commands, messages from peers, and the time; its outputs are messages to send and notices of what changed.
- The core starts no threads, opens no connections, and brings no runtime. It does nothing between inputs.
- The core never reads a clock or generates randomness. Both are inputs, supplied by the application.
- The same inputs in the same order produce the same outputs and the same stored state, on every target.
- Bindings are thin. A browser binding, a foreign-function binding, and a native program differ in how they deliver inputs and perform outputs, and in nothing else.

## Steps

- The core takes a batch of inputs and handles it in one step.
- Everything a step changes is written to [storage](storage.md) as one atomic batch.
- A step's outputs are released only after its batch is durable. Nothing is ever said to a peer that the device could forget.
- This ordering is a safety property of [agreement](checkpoints.md), not a data-loss precaution, and no binding may relax it.
- A step does bounded work. Anything long, such as catching up, settling a large checkpoint, or evaluating a wide [commit operation](commit-operations.md), continues across steps.
- A step that leaves work which waits for nothing says so in its outputs, and the application steps the core again. The core never steps itself.
- A [checkpoint](checkpoints.md) committed across several steps is committed in [timeline](log.md) order. Between those steps a committed [read](reads.md) shows part of it, all of which is final.
- A snapshot, once whole and verified, is put in place across several steps in key order. Between those steps a committed read shows each value as of the snapshot's checkpoint or as of the one the device was at.
- Until the last of either is done, the device has the state of no checkpoint: it neither [compacts](compaction.md) nor serves a snapshot, and commits nothing further.
- [Shedding](compaction.md) history is a decision, made in one step: from then on the device answers as one that no longer has it. Deleting it follows across steps, and nothing waits for that.
- A device partway through putting a snapshot in place can stop [wanting](sync.md) a clock, but cannot start wanting one the snapshot has passed over.
- A device may stop between any two steps and resume from storage alone.

## Storage is awaited, not requested

- The core reads and writes storage directly, during a step, and waits for the answer where it asked.
- Evaluating anything at a place in the [timeline](log.md) means reading state: a guard, a commit operation, a derived index.
- Were each read a request answered by a later input, every such feature would be a suspension written by hand. The core would spend its complexity on waiting.
- A step waits on storage and on nothing else. With a store that answers at once, a step never waits at all.
- Storage is not required to answer at once. A core that demanded it could not use a browser's database, and would be confined to a worker there.
- A step runs alone. Inputs that arrive while one is waiting are handled by the next.
- A step abandoned while waiting leaves the core unusable until it is reloaded from storage. Nothing was said, so nothing is lost.
- The network stays outside the core because it is slow, unreliable, and the thing worth simulating. Storage is none of those once it is behind a contract.

## Reads

- A [read](reads.md) runs against a consistent view of storage and does not pass through the state machine.
- A read sees whole steps: all of a step's writes, or none.
- A change notice is released with the step that caused it, after its batch, so a reader acting on one sees that step's writes.

## Time

- Time is used only to wait: between attempts at a checkpoint, and before retrying a peer.
- Time never orders data, breaks a tie, or decides validity.
- A device with a wrong clock is slow or impatient, never incorrect.

## Testing

- Every protocol property is tested by simulation: many cores, an in-memory store each, and a network that delays, drops, repeats, and reorders.
- The in-memory store answers at once, so a simulated step is one uninterrupted computation and its outcome depends on inputs alone.
- Simulation controls every input, so a failure reproduces from its inputs alone.
- Simulated faults include stopping a device between steps and losing a device's store.
- A property that cannot be stated as a simulation is not yet understood well enough to build.

## Ownership

- Exactly one core runs against a store at a time. Two would each be the same peer, and could contradict each other.
- The same holds for the network: one peer is one endpoint.
- An application with several windows or processes elects one to own the core, and routes the rest through it.
- A store that can tell it already has an owner refuses a second.

## Transport

- The core addresses peers by key and hands the application bytes to deliver. See [sync](sync.md) for what the transport must provide.
- The core is designed for Iroh and does not depend on it.
