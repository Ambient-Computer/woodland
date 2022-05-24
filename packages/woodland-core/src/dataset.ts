import type { CID } from 'multiformats/cid';
import { v4 as uuid } from 'uuid';
import { createHash, assert } from './utils';
import type { StoragePlugin, StorageBackend } from './backend/interface';
import {
  type Instruction,
  type Message,
  InstructionType,
} from './replication/interface';
import {
  type GenesisBlock,
  type HistoricalRecord,
  HistoricalRecordType,
} from './types';

export default class Dataset {
  private backend: StorageBackend;

  /** A unique, persistent UUID which globally identifies the dataset. */
  public id: Uint8Array;

  /**
   * Generates a new, distinct dataset. It can only be merged with other
   * datasets that share a common history.
   */
  static async create(config: Config): Promise<Dataset> {
    const genesis: GenesisBlock = {
      id: uuid({}, new Uint8Array(16)),
    };

    // The UUID would do well enough, but using a CID keeps consistent size.
    const [cid, backend] = await Promise.all([
      createHash(genesis.id),
      config.storage.open(genesis.id),
    ]);

    await backend
      .createTransaction()
      .addTransaction(cid, genesis)
      .appendToHistory(createLocalChangeEvent(cid))
      .commit();

    return new Dataset(genesis.id, backend);
  }

  /**
   * Load an existing dataset. Don't open more than one instance of a single
   * dataset. Unexpected things may happen.
   */
  static async open(datasetId: Uint8Array, config: Config): Promise<Dataset> {
    const [backend, genesisId] = await Promise.all([
      config.storage.open(datasetId),
      createHash(datasetId),
    ]);

    assert(await backend.readTransaction(genesisId), 'Dataset does not exist.');

    return new Dataset(datasetId, backend);
  }

  private constructor(id: Uint8Array, backend: StorageBackend) {
    this.backend = backend;
    this.id = id;
  }

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

  /** Receive and process an instruction from a remote dataset. */
  async execute<Type extends InstructionType>(
    instruction: Instruction<Type>,
  ): Promise<Message> {
    if (instruction?.type && this.handlers.hasOwnProperty(instruction.type)) {
      return this.handlers[instruction.type](instruction);
    }

    // Invalid message.
    return {
      reply: { type: InstructionType.Quit },
      broadcast: { type: InstructionType.Quit },
    };
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

  private handlers: InstructionHandlers = {
    [InstructionType.Quit]: () => ({
      reply: { type: InstructionType.Quit },
      broadcast: { type: InstructionType.Quit },
    }),
    [InstructionType.Integrate]: () => ({
      reply: { type: InstructionType.Quit },
      broadcast: { type: InstructionType.Quit },
    }),
    [InstructionType.FindCommonAncestor]: () => ({
      reply: { type: InstructionType.Quit },
      broadcast: { type: InstructionType.Quit },
    }),
    [InstructionType.ReceiveAncestors]: () => ({
      reply: { type: InstructionType.Quit },
      broadcast: { type: InstructionType.Quit },
    }),
  };
}

interface Config {
  /** A storage plugin. */
  storage: StoragePlugin;
}

interface InsertResult {
  operationId: CID;
  transactionId: CID;
  historicalRecord: HistoricalRecord;
}

type InstructionHandlers = {
  [Type in InstructionType]: (instruction: Instruction<Type>) => Message;
};

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
