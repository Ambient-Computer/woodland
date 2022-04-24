import type { CID } from 'multiformats/cid';
import type { StorageBackend, TransactionBuilder } from './interface';

export default class MemoryBackend implements StorageBackend {
  private store = createStore();

  public createTransaction() {
    return new MemoryTransactionBuilder(this.store);
  }

  public async readOperation(id: CID): Promise<null | ArrayBuffer> {
    return this.store.operations.get(id.toString()) || null;
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
  }

  public async commit() {
    // Apply all the operations.
    for (const [key, value] of this.pending.operations) {
      this.store.operations.set(key, value);
    }
  }
}

function createStore(): Store {
  return {
    operations: new Map(),
  };
}

interface Store {
  operations: Map<string, ArrayBuffer>;
}
