import type { CID } from 'multiformats/cid';
import { stringify } from 'uuid';
import type {
  StoragePlugin,
  StorageBackend,
  TransactionBuilder,
  TransactionRunner,
  ObjectStore,
  ObjectStores,
  HistoryStore,
} from './interface';
import type { HistoricalRecord, Transaction } from '../types';
import { assert } from '../utils';

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
  constructor(private store: Store) {}

  public createTransaction() {
    return new MemoryTransactionBuilder(this.store);
  }

  public async createWritableTransaction<T>(runner: TransactionRunner<T>) {
    const builder = new MemoryTransactionBuilder(this.store);
    const result = await runner(builder);
    builder.commit();

    return result;
  }

  public async readOperation(id: CID) {
    return this.store.operations.get(id.toString()) || null;
  }

  public async readTransaction(id: CID) {
    return this.store.transactions.get(id.toString()) || null;
  }

  public async getMostRecentEvent(): Promise<HistoricalRecord> {
    const record = this.store.history[this.store.history.length - 1];
    assert(record, 'History is empty. This should never happen.');

    return record;
  }
}

class MemoryTransactionBuilder implements TransactionBuilder, ObjectStores {
  private pending = createStore();

  constructor(private store: Store) {}

  public addOperation(id: CID, operation: ArrayBuffer) {
    this.pending.operations.set(id.toString(), operation);
    return this;
  }

  public addTransaction(id: CID, record: Transaction) {
    this.pending.transactions.set(id.toString(), record);
    return this;
  }

  public appendToHistory(record: HistoricalRecord) {
    this.pending.history.push(record);
    return this;
  }

  public async commit() {
    // Apply all the operations.
    for (const [key, operation] of this.pending.operations) {
      this.store.operations.set(key, operation);
    }

    // Apply all the transactions.
    for (const [key, record] of this.pending.transactions) {
      this.store.transactions.set(key, record);
    }

    this.store.history.push(...this.pending.history);
  }

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
