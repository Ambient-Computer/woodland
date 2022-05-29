import type { CID } from 'multiformats/cid';
import { v4 as uuid } from 'uuid';
import assert from 'assert';
import createHash from './utils/hash';
import type { StorageBackend, StoragePlugin } from './backend/interface';
import type { GenesisBlock, HistoricalRecord } from './types';
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
    const [cid, storageBackend] = await Promise.all([
      createHash(genesis.id),
      storage.open(genesis.id),
    ]);

    await storageBackend
      .createTransaction()
      .addTransaction(cid, genesis)
      .appendToHistory(createLocalChangeEvent(cid))
      .commit();

    return {
      controller: new BackendController(storageBackend),
      genesis,

      // TODO: Remove this when the backend controller fully supplants
      // imperative controls.
      backend: storageBackend,
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

    assert(await backend.readTransaction(genesisId), 'Dataset does not exist.');

    return {
      controller: new BackendController(backend),

      // TODO: Remove when the controller supplants imperative storage
      // operations.
      backend,
    };
  }

  private constructor(private backend: StorageBackend) {}

  async insertOperation(operation: ArrayBuffer): Promise<InsertResult> {
    const [{ head: parentId }, operationId] = await Promise.all([
      this.backend.getMostRecentEvent(),
      createHash(operation),
    ]);

    const [index, transactionId] = await Promise.all([
      this.computeNextIndex(parentId),
      createTransactionId(parentId, operationId),
    ]);

    const historicalRecord = createLocalChangeEvent(transactionId);
    const record = { parentId, predecessorId: null, operationId, index };

    await this.backend
      .createTransaction()
      .addOperation(operationId, operation)
      .addTransaction(transactionId, record)
      .appendToHistory(historicalRecord)
      .commit();

    return { operationId, transactionId, historicalRecord };
  }

  private async computeNextIndex(recordId: CID): Promise<number> {
    const record = await this.backend.readTransaction(recordId);
    assert(record, 'Record does not exist.');

    if ('index' in record) {
      return record.index + 1;
    }

    // The parent is a genesis block.
    return 1;
  }
}

interface InsertResult {
  operationId: CID;
  transactionId: CID;
  historicalRecord: HistoricalRecord;
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
