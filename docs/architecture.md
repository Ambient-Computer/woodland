# Woodland Architecture

An append-only replicated operation log similar to [Timber](https://github.com/PsychoLlama/timber-protocol), but without compaction.

The design is crash-only where reasonable.

## API

Construct a log:

```typescript
const log = OperationLog.create({
  backend: IndexedDBDriver,
  schema: Schema,
  migrations,
  version: migrations[-1].id,
})
```

Then derive state from the log:

```typescript
const views = log.defineViews(schema => {
  counter: {
    initialState: { count: 0 },
    inputs: [schema.increment, schema.decrement],
    reducer: (state, operation) => {
      if (operation.type === 'increment') {
        state.count += operation.payload
      }

      if (operation.type === 'decrement') {
        state.count -= operation.payload
      }
    },
  },
})

// Async, because we may need to consult the storage backend.
await views.counter.getState() // => 0
log.view.counter.subscribe((state) => {
  // Watch the state as it changes.
})
```

The reducer is called for every new operation. If an update interleaves itself in older history, we roll back to that point, apply the update, then roll forward again. Snapshots are periodically stored in the database to avoid recomputing the entire log on startup.

## Types

Valid operations are defined ahead of time. The payload must be a JSON subset. Operation types are versioned: any new fields or changes to the existing structure are additive, not mutative.

Operation objects must be immutable, changed through versioned schemas. Possibly content-addressed. I haven't figured out that bit yet.

TODO: figure out that bit.

```typescript
import { defineSchema } from 'woodland'

defineSchema(t => {
  noParameters: t.none,
  increment: t.number,
  decrement: t.number,
  custom: t.struct({
    list: t.listOf(t.string),
    map: t.mapOf(t.number),
  })
})
```

## Making Changes

Types are instantiated from the definitions in the schema. They are applied in bulk.

```typescript
const { delta, promise } = log.append(actions => [
  actions.noParameters(),
  actions.increment(1),
  actions.increment(5),
  actions.custom({
    list: ['first', 'second'],
    map: {
      a: 5,
      b: 10,
    },
  }),
])
```

The delta is an opaque set of operations to be merged into remote causal trees.

## Protocol

Deltas are exchanged and synchronized through an interactive stateless protocol provided by imperative APIs. You've already seen the `delta` value returned from `.append(...)`, that's the first side.

The underlying structure resembles this:

```typescript
interface Operation<T> {
  // ID of the payload type.
  type: string;
  // ID of the previous operation.
  prev: string;
  // Migration version ID.
  version: string;
  // Typed payload.
  payload: T;
}

interface Delta {
  operations: Map<string, Operation>;
}
```

Each operation is content addressed to avoid conflicts, both accidental and malicious.

## Merging

Merge operations are a set union. Orphaned operations are not joined until the predecessors are given, which must be negotiated by the network.

```typescript
function merge(a: State, b: State) {
  return { ...a, ...b }
}
```

At a high level, for every new operation we:

- Check to see if the ID already exists in state. If so, break.
- Verify the checksum, preventing malicious actors from intentionally causing conflicts.
- Run any unapplied migrations.
- Derive new state.

## Migrations

Migrations are simple functions that map one operation to another, deterministically applied at runtime. They do not mutate the source log.

There are two types of migrations: staged and static. Staged migrations are applied on the fly in memory, meaning you can test migrations in production without risking data corruption. Once you're certain the migration is safe, changing to static will cause the log to be completely rebuilt, bringing all operations fully up to date.

Migrations run in a background worker. For static migrations, if an operation is changed, record a mapping between the old ID (hash) to the new ID (hash) and store it in a migration database.

New actions default to the version of the last static migration(?).

### Backward Compatibility

New types and migrations should be deployed well before they're actually used. If a client receives a type they don't recognize, they'll reject it which would create a type of partition where older clients are only able to accept operations from other clients of the same release. The new clients will be able to continuously integrate older updates, but older clients will be unaware of the newer states. Everything will reconcile once the older clients update.

## Disk Format

Here is a hypothetical structure for a persisted backend.

### Operations

The causal tree is a grow-only set of operations mapped by ID. Content-addressed data prevents namespace conflicts.

```typescript
interface Operation<T> {
  // ...other fields...
  id: string;
  prev: string;
  payload: T;
}

type Operations = Map<ID, Operation>
```

The causal tree is ordered by following the operations as a linked list, sorting concurrent values deterministically by ID.

The backend must support pagination and the ability to start from an arbitrary ID.

### Logs

There can be more than one instance of a causal tree simultaneously if undergoing a static migration. We need a way to gracefully transition between the two which survives crashes. A table seems appropriate.

```typescript
type Logs = Map<MigrationID, Operations>
```

### Migrations

TODO: flesh this out.

### Snapshots

TODO: figure out DB structure for view snapshots. May require a schema.
