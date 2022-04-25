import type { CID } from 'multiformats/cid';
import type { StorageBackend } from './backend/interface';
import { type HistoricalRecord, HistoricalRecordType } from './types';
import { createHash } from './utils';

export default class Dataset {
  private backend: StorageBackend;

  static async create(config: Config): Promise<Dataset> {
    return new Dataset(config);
  }

  private constructor(config: Config) {
    this.backend = config.backend;
  }

  async insertOperation(operation: ArrayBuffer): Promise<InsertResult> {
    const cid = await createHash(operation);
    const historicalRecord: HistoricalRecord = {
      type: HistoricalRecordType.LocalChange,
      timestamp: new Date().toISOString(),

      // TODO: Replace these with TL CIDs. This points to an operation.
      changeset: [cid],
      timeline: cid,
    };

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
