import type { CID } from 'multiformats/cid';
import type { StorageBackend, TransactionBuilder } from './interface';
import type { HistoricalRecord, TimelineRecord } from '../types';
import { assert } from '../utils';

export default class MemoryBackend implements StorageBackend {
  private store = createStore();

  public createTransaction() {
    return new MemoryTransactionBuilder(this.store);
  }

  public async readOperation(id: CID) {
    return this.store.operations.get(id.toString()) || null;
  }

  public async readTimelineRecord(id: CID) {
    return this.store.timelines.get(id.toString()) || null;
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

  public addTimelineRecord(id: CID, record: TimelineRecord) {
    this.pending.timelines.set(id.toString(), record);
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

    // Apply all the timelines.
    for (const [key, record] of this.pending.timelines) {
      this.store.timelines.set(key, record);
    }

    this.store.history.push(...this.pending.history);
  }
}

function createStore(): Store {
  return {
    operations: new Map(),
    timelines: new Map(),
    history: [],
  };
}

interface Store {
  operations: Map<string, ArrayBuffer>;
  timelines: Map<string, TimelineRecord>;
  history: Array<HistoricalRecord>;
}
