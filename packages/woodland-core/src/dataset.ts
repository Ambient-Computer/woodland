import type { CID } from 'multiformats/cid';
import { v4 as uuid } from 'uuid';
import { createHash, assert } from './utils';
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

  // TODO: Force serial execution to avoid issues with the changelog.
  async insertOperation(operation: ArrayBuffer): Promise<InsertResult> {
    const [{ timeline: parentId }, operationId] = await Promise.all([
      this.backend.getMostRecentEvent(),
      createHash(operation),
    ]);

    const [index, timelineId] = await Promise.all([
      this.computeNextIndex(parentId),
      createTimelineId(parentId, operationId),
    ]);

    const historicalRecord = createLocalChangeEvent(timelineId);
    const record = { parentId, predecessorId: null, operationId, index };

    await this.backend
      .createTransaction()
      .addOperation(operationId, operation)
      .addTimelineRecord(timelineId, record)
      .appendToHistory(historicalRecord)
      .commit();

    return { operationId, timelineId, historicalRecord };
  }

  private async computeNextIndex(recordId: CID): Promise<number> {
    const record = await this.backend.readTimelineRecord(recordId);
    assert(record, 'Record does not exist.');

    if ('index' in record) {
      return record.index + 1;
    }

    // The parent is a genesis block.
    return 1;
  }
}

interface Config {
  /** A storage plugin. */
  backend: StorageBackend;
}

interface InsertResult {
  operationId: CID;
  timelineId: CID;
  historicalRecord: HistoricalRecord;
}

function createLocalChangeEvent(timelineId: CID): HistoricalRecord {
  return {
    type: HistoricalRecordType.LocalChange,
    timestamp: new Date().toISOString(),
    changeset: [timelineId],
    timeline: timelineId,
  };
}

// hash(lastRecordId + newOperationId)
function createTimelineId(lastId: CID, operationId: CID): Promise<CID> {
  const src = new Uint8Array(lastId.bytes.length + operationId.bytes.length);

  src.set(lastId.bytes, 0);
  src.set(operationId.bytes, lastId.bytes.byteLength);

  return createHash(src);
}
