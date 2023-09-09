import type { CID } from 'multiformats/cid';
import { stringify } from 'uuid';
import type { HistoricalRecord, Transaction } from '../types';
import { HistoricalRecordType } from '../types';
import { assert } from '../utils';
import type {
  StoragePlugin,
  StorageBackend,
  TransactionRunner,
  ObjectStore,
  ObjectStores,
  HistoryStore,
} from './interface';
import MemoryTimeline from './timeline/memory';

export default class MemoryStoragePlugin implements StoragePlugin {
  private databases: Map<string, Store> = new Map();

  async open(datasetId: Uint8Array) {
    const uuid = stringify(datasetId);
    const store = this.getOrCreateStore(uuid);
    return new MemoryBackend(store);
  }

  private getOrCreateStore(datasetId: string) {
    const existingStore = this.databases.get(datasetId);
    if (existingStore) return existingStore;

    const store = createStore();
    this.databases.set(datasetId, store);
    return store;
  }
}

class MemoryBackend implements StorageBackend {
  private closed = false;

  constructor(private store: Store) {}

  public async createTransaction<T>(runner: TransactionRunner<T>) {
    assert(!this.closed, 'Cannot open transaction. Storage backend is closed.');

    const pending = createStore();
    const stores = new MemoryTransactionBuilder(pending, this.store);

    const result: T = await runner(stores);
    this.commit(pending);

    return result;
  }

  private async commit(pending: Store) {
    // Apply all the operations.
    for (const [key, operation] of pending.operations) {
      this.store.operations.set(key, operation);
    }

    // Apply all the transactions.
    for (const [key, record] of pending.transactions) {
      this.store.transactions.set(key, record);
    }

    this.store.history.push(...pending.history);
  }

  async close() {
    this.closed = true;
  }
}

class MemoryTransactionBuilder implements ObjectStores {
  constructor(
    private pending: Store,
    private store: Store,
  ) {}

  public operations = new MemoryObjectStore<ArrayBuffer>(
    this.pending.operations,
    this.store.operations,
  );

  public transactions = new MemoryObjectStore<Transaction>(
    this.pending.transactions,
    this.store.transactions,
  );

  public history = new MemoryHistoryStore(
    this.pending.history,
    this.store.history,
  );
}

class MemoryObjectStore<T> implements ObjectStore<T> {
  constructor(
    private pending: Map<string, T>,
    private committed: Map<string, T>,
  ) {}

  async get(id: CID) {
    const serializedId = id.toString();

    return (
      this.pending.get(serializedId) || this.committed.get(serializedId) || null
    );
  }

  async put(id: CID, value: T) {
    this.pending.set(id.toString(), value);
  }
}

class MemoryHistoryStore implements HistoryStore {
  constructor(
    private pending: Array<HistoricalRecord>,
    private committed: Array<HistoricalRecord>,
  ) {}

  async append(record: HistoricalRecord) {
    this.pending.push(record);
  }

  async getMostRecentEvent() {
    const allRecords = this.committed.concat(this.pending);
    const record = allRecords[allRecords.length - 1];
    assert(record, 'History is empty. This should never happen.');

    return record;
  }

  async get(id: number) {
    const allRecords = this.committed.concat(this.pending);
    return allRecords[id] ?? null;
  }

  async put(id: number, record: HistoricalRecord) {
    assert(
      id >= this.committed.length,
      'You may only update pending changelog records.',
    );

    this.pending[id - this.committed.length] = record;
  }

  async initialize(genesisId: CID): Promise<HistoricalRecord> {
    const record: HistoricalRecord = {
      type: HistoricalRecordType.LocalChange,
      timestamp: new Date().toISOString(),
      changeset: [genesisId],
      head: genesisId,
      timeline: MemoryTimeline.withEntries([genesisId]),
    };

    await this.put(0, record);

    return record;
  }
}

function createStore(): Store {
  return {
    operations: new Map(),
    transactions: new Map(),
    history: [],
  };
}

interface Store {
  operations: Map<string, ArrayBuffer>;
  transactions: Map<string, Transaction>;
  history: Array<HistoricalRecord>;
}
