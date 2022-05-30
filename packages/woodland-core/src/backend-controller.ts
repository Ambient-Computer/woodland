import type { CID } from 'multiformats/cid';
import { v4 as uuid } from 'uuid';
import assert from 'assert';
import { createHash, hashIds, hashTransaction } from './utils';
import type {
  StorageBackend,
  StoragePlugin,
  ObjectStores,
} from './backend/interface';
import {
  GenesisBlock,
  HistoricalRecord,
  InsertionDelta,
  HistoricalRecordType,
  Record,
} from './types';

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
    const [cid, backend] = await Promise.all([
      createHash(genesis.id),
      storage.open(genesis.id),
    ]);

    await backend.createTransaction(async (stores) => {
      stores.transactions.put(cid, genesis);
      stores.history.append(createLocalChangeEvent(cid));
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

  async insertOperation(operation: ArrayBuffer): Promise<InsertionDelta> {
    return this.backend.createTransaction(async (stores) => {
      const [{ head: parentId }, operationId] = await Promise.all([
        stores.history.getMostRecentEvent(),
        createHash(operation),
      ]);

      const [index, transactionId] = await Promise.all([
        this.computeNextIndex(parentId, stores),
        hashIds([parentId, operationId]),
      ]);

      const historicalRecord = createLocalChangeEvent(transactionId);
      const record = { parentId, predecessorId: null, operationId, index };

      stores.operations.put(operationId, operation);
      stores.transactions.put(transactionId, record);
      stores.history.append(historicalRecord);

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
        stores.history.put(0, createLocalChangeEvent(genesisId)),
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
      const allOperations = Array.from(update.operations.values());

      const [transactionIds] = await Promise.all([
        this.saveTransactions(stores, update.transactions),
        this.saveOperations(stores, allOperations),
      ]);

      await stores.history.append({
        type: HistoricalRecordType.RemoteChange,
        timestamp: new Date().toISOString(),
        head: transactionIds[transactionIds.length - 1],
        changeset: transactionIds,
      });
    });
  }

  private async saveOperations(
    stores: ObjectStores,
    operations: Array<ArrayBuffer>,
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

function createLocalChangeEvent(transactionId: CID): HistoricalRecord {
  return {
    type: HistoricalRecordType.LocalChange,
    timestamp: new Date().toISOString(),
    changeset: [transactionId],
    head: transactionId,
  };
}
