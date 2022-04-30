import type { CID } from 'multiformats/cid';
import { stringify } from 'uuid';
import type {
  StoragePlugin,
  StorageBackend,
  TransactionBuilder,
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
  private store: Store;

  constructor(store: Store) {
    this.store = store;
  }

  public createTransaction() {
    return new MemoryTransactionBuilder(this.store);
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

class MemoryTransactionBuilder implements TransactionBuilder {
  private pending = createStore();
  private store;

  constructor(store: Store) {
    this.store = store;
  }

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
