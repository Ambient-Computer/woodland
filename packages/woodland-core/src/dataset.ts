import type { CID } from 'multiformats/cid';
import { v4 as uuid } from 'uuid';
import { createHash } from './utils';
import type { StorageBackend } from './backend/interface';
import {
  type GenesisBlock,
  type HistoricalRecord,
  HistoricalRecordType,
} from './types';

export default class Dataset {
  private backend: StorageBackend;

  /**
   * Generates a new, distinct dataset. It can only be merged with other
   * datasets that share a common history.
   */
  static async create(config: Config): Promise<Dataset> {
    const genesis: GenesisBlock = {
      id: uuid({}, new Uint8Array(16)),
    };

    // The UUID would do well enough, but using a CID keeps consistent size.
    const cid = await createHash(genesis.id);

    await config.backend
      .createTransaction()
      .addTimelineRecord(cid, genesis)
      .appendToHistory(createLocalChangeEvent(cid))
      .commit();

    return new Dataset(config);
  }

  private constructor(config: Config) {
    this.backend = config.backend;
  }

  async insertOperation(operation: ArrayBuffer): Promise<InsertResult> {
    const cid = await createHash(operation);

    // TODO: Replace this with TL CIDs. This points to an operation.
    const historicalRecord = createLocalChangeEvent(cid);

    // TODO: Consider concurrent `insertOperation(...)` merging with history.
    // Possibly force serial execution.
    await this.backend
      .createTransaction()
      .addOperation(cid, operation)
      .appendToHistory(historicalRecord)
      .commit();

    return {
      operationId: cid,
      historicalRecord,
    };
  }
}

interface Config {
  /** A storage plugin. */
  backend: StorageBackend;
}

interface InsertResult {
  operationId: CID;
  historicalRecord: HistoricalRecord;
}

function createLocalChangeEvent(recordId: CID): HistoricalRecord {
  return {
    type: HistoricalRecordType.LocalChange,
    timestamp: new Date().toISOString(),
    changeset: [recordId],
    timeline: recordId,
  };
}
