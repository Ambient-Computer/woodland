import type { CID } from 'multiformats/cid';
import { v4 as uuid } from 'uuid';
import assert from 'assert';
import createHash from './utils/hash';
import type {
  StorageBackend,
  StoragePlugin,
  ObjectStores,
} from './backend/interface';
import type { GenesisBlock, HistoricalRecord, InsertionDelta } from './types';
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
    const [cid, backend] = await Promise.all([
      createHash(genesis.id),
      storage.open(genesis.id),
    ]);

    await backend.createTransaction(async (stores) => {
      stores.transactions.put(cid, genesis);
      stores.history.append(createLocalChangeEvent(cid));
    });

    return {
      controller: new BackendController(backend),
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

      return new BackendController(backend);
    });
  }

  private constructor(private backend: StorageBackend) {}

  async insertOperation(operation: ArrayBuffer): Promise<InsertionDelta> {
    return this.backend.createTransaction(async (stores) => {
      const [{ head: parentId }, operationId] = await Promise.all([
        stores.history.getMostRecentEvent(),
        createHash(operation),
      ]);

      const [index, transactionId] = await Promise.all([
        this.computeNextIndex(parentId, stores),
        createTransactionId(parentId, operationId),
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
}

function createLocalChangeEvent(transactionId: CID): HistoricalRecord {
  return {
    type: HistoricalRecordType.LocalChange,
    timestamp: new Date().toISOString(),
    changeset: [transactionId],
    head: transactionId,
  };
}

// hash(lastRecordId + newOperationId)
function createTransactionId(lastId: CID, operationId: CID): Promise<CID> {
  const src = new Uint8Array(lastId.bytes.length + operationId.bytes.length);

  src.set(lastId.bytes, 0);
  src.set(operationId.bytes, lastId.bytes.byteLength);

  return createHash(src);
}
