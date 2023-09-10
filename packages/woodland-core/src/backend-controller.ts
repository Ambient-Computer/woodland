import assert from 'assert';
import type { CID } from 'multiformats/cid';
import { v4 as uuid } from 'uuid';
import { createHash, hashIds, hashTransaction } from './utils';
import type {
  StorageBackend,
  StoragePlugin,
  ObjectStores,
  Timeline,
} from './backend/interface';
import type {
  GenesisBlock,
  HistoricalRecord,
  InsertionDelta,
  Record,
} from './types';
import { HistoricalRecordType } from './types';

/**
 * Manages queries and writes over the dataset backend.
 */
export default class BackendController {
  /**
   * Generate a new ID for a dataset and initialize log state in the storage
   * backend.
   */
  static async initializeDataset(storage: StoragePlugin) {
    const genesis: GenesisBlock = {
      id: uuid({}, new Uint8Array(16)),
    };

    // The UUID would do well enough, but using a CID keeps consistent size.
    const [genesisId, backend] = await Promise.all([
      createHash(genesis.id),
      storage.open(genesis.id),
    ]);

    await backend.createTransaction(async (stores) => {
      stores.transactions.put(genesisId, genesis);
      stores.history.initialize(genesisId);
    });

    return {
      controller: new BackendController(genesis.id, backend),
      genesis,
    };
  }

  /**
   * Open an existing dataset. Fails if the dataset hasn't been initialized.
   * Depending on the storage backend, multiple open instances of the same
   * dataset are allowed.
   */
  static async openDataset(config: {
    storage: StoragePlugin;
    datasetId: Uint8Array;
  }) {
    const [backend, genesisId] = await Promise.all([
      config.storage.open(config.datasetId),
      createHash(config.datasetId),
    ]);

    return backend.createTransaction(async (stores) => {
      assert(
        await stores.transactions.get(genesisId),
        'Dataset does not exist.',
      );

      return new BackendController(config.datasetId, backend);
    });
  }

  private constructor(
    private readonly id: Uint8Array,
    private backend: StorageBackend,
  ) {}

  async insertOperation(operation: Uint8Array): Promise<InsertionDelta> {
    return this.backend.createTransaction(async (stores) => {
      const [{ head: parentId, timeline }, operationId] = await Promise.all([
        stores.history.getMostRecentEvent(),
        createHash(operation),
      ]);

      const [index, transactionId] = await Promise.all([
        this.computeNextIndex(parentId, stores),
        hashIds([parentId, operationId]),
      ]);

      const historicalRecord = createLocalChangeEvent(transactionId, timeline);
      const record = { parentId, predecessorId: null, operationId, index };

      stores.operations.put(operationId, operation);
      stores.transactions.put(transactionId, record);
      stores.history.append(historicalRecord);
      timeline.append([transactionId]);

      return {
        head: transactionId,
        transactions: [record],
        operations: new Map([[operationId, operation]]),
      };
    });
  }

  /**
   * Clones the root transaction to another storage backend and returns a new
   * controller. This is useful for testing replication with another memory
   * backend or for migrating an existing database to a different provider.
   */
  async createEmptyClone(plugin: StoragePlugin): Promise<BackendController> {
    const backend = await plugin.open(this.id);
    const genesisId = await createHash(this.id);

    return backend.createTransaction(async (stores) => {
      await Promise.all([
        stores.transactions.put(genesisId, { id: this.id }),
        stores.history.initialize(genesisId),
      ]);

      return new BackendController(this.id, backend);
    });
  }

  /**
   * Integrate a changeset from a remote dataset.
   *
   * Assumptions:
   * - The object is valid
   * - Transactions are sequential
   * - Transactions build on our latest change (FIXME)
   */
  async integrate(update: InsertionDelta) {
    await this.backend.createTransaction(async (stores) => {
      const allOperations = Array.from(
        update.operations.values(),
        (buffer) => new Uint8Array(buffer),
      );

      const [{ timeline }, transactionIds] = await Promise.all([
        stores.history.getMostRecentEvent(),
        this.saveTransactions(stores, update.transactions),
        this.saveOperations(stores, allOperations),
      ]);

      await stores.history.append({
        type: HistoricalRecordType.RemoteChange,
        timestamp: new Date().toISOString(),
        head: transactionIds[transactionIds.length - 1],
        changeset: transactionIds,
        timeline: timeline.append(transactionIds),
      });
    });
  }

  private async saveOperations(
    stores: ObjectStores,
    operations: Array<Uint8Array>,
  ) {
    const promises = operations.map(async (operation) => {
      const hash = await createHash(operation);
      stores.operations.put(hash, operation);
    });

    await Promise.all(promises);
  }

  private async saveTransactions(
    stores: ObjectStores,
    transactions: Array<Record>,
  ) {
    const promises = transactions.map(async (transaction) => {
      const transactionId = await hashTransaction(transaction);
      stores.transactions.put(transactionId, transaction);

      return transactionId;
    });

    return Promise.all(promises);
  }

  private async computeNextIndex(
    recordId: CID,
    stores: ObjectStores,
  ): Promise<number> {
    const record = await stores.transactions.get(recordId);
    assert(record, 'Record does not exist.');

    if ('index' in record) {
      return record.index + 1;
    }

    // The parent is a genesis block.
    return 1;
  }

  async close() {
    await this.backend.close();
  }
}

function createLocalChangeEvent(
  transactionId: CID,
  timeline: Timeline,
): HistoricalRecord {
  return {
    type: HistoricalRecordType.LocalChange,
    timestamp: new Date().toISOString(),
    changeset: [transactionId],
    head: transactionId,
    timeline: timeline.append([transactionId]),
  };
}
