# Library

Woodland is a library with a deterministic core. The application owns the process, the network, the clock, and the keys; the core owns the protocol and the data.

## Shape

- The core is a state machine. Its inputs are local commands, messages from peers, and the time; its outputs are messages to send and notices of what changed.
- The core starts no threads, opens no connections, and needs no runtime. It does nothing between inputs.
- The core never reads a clock or generates randomness. Both are inputs, supplied by the application.
- The same inputs in the same order produce the same outputs and the same stored state, on every target.
- Bindings are thin. A browser binding, a foreign-function binding, and a native program differ in how they deliver inputs and perform outputs, and in nothing else.

## Steps

- The core takes a batch of inputs and handles it in one step.
- Everything a step changes is written to [storage](storage.md) as one atomic batch.
- A step's outputs are released only after its batch is durable. Nothing is ever said to a peer that the device could forget.
- This ordering is a safety property of [agreement](checkpoints.md), not a data-loss precaution, and no binding may relax it.
- A step does bounded work. Anything long, such as catching up, settling a large checkpoint, or evaluating a wide [commit operation](commit-operations.md), continues across steps.
- A device may stop between any two steps and resume from storage alone.

## Storage is called, not requested

- The core reads and writes storage directly and synchronously, during a step.
- Evaluating anything at a place in the [timeline](log.md) means reading state: a guard, a commit operation, a derived index.
- Were each read a request answered by a later input, every such feature would be a hand-written suspension. The core would spend its complexity on waiting.
- The network stays outside the core because it is slow, unreliable, and the thing worth simulating. Storage is none of those once it is behind a contract.

## Reads

- A [read](reads.md) runs against a consistent view of storage and does not pass through the state machine.
- A read never blocks a step, and a step never invalidates a read in progress.
- A change notice is released with the step that caused it, after its batch, so a reader acting on one sees that step's writes.

## Time

- Time is used only to wait: between attempts at a checkpoint, and before retrying a peer.
- Time never orders data, breaks a tie, or decides validity.
- A device with a wrong clock is slow or impatient, never incorrect.

## Testing

- Every protocol property is tested by simulation: many cores, an in-memory store each, and a network that delays, drops, repeats, and reorders.
- Simulation controls every input, so a failure reproduces from its inputs alone.
- Simulated faults include stopping a device between steps and losing a device's store.
- A property that cannot be stated as a simulation is not yet understood well enough to build.

## Ownership

- Exactly one core runs against a store at a time. Two would each be the same peer, and could contradict each other.
- An application with several windows or processes routes them through the one that owns the store.

## Transport

- The core addresses peers by key and hands the application bytes to deliver. See [sync](sync.md) for what the transport must provide.
- The core is designed for Iroh and does not depend on it.
