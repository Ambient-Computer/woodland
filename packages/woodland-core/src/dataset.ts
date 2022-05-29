import type { CID } from 'multiformats/cid';
import type { StoragePlugin } from './backend/interface';
import type { Instruction, Message } from './replication/interface';
import type { HistoricalRecord } from './types';
import { InstructionType } from './replication/interface';
import BackendController from './backend-controller';

export default class Dataset {
  /**
   * Generates a new, distinct dataset. It can only be merged with other
   * datasets that share a common history.
   */
  static async create(config: Config): Promise<Dataset> {
    const { controller, genesis } = await BackendController.initializeDataset(
      config.storage,
    );

    return new Dataset(genesis.id, controller);
  }

  /** Load an existing dataset. */
  static async open(datasetId: Uint8Array, config: Config): Promise<Dataset> {
    const { controller } = await BackendController.openDataset({
      storage: config.storage,
      datasetId,
    });

    return new Dataset(datasetId, controller);
  }

  private constructor(
    /** A unique, persistent UUID which globally identifies the dataset. */
    public id: Uint8Array,
    private backend: BackendController,
  ) {}

  async insertOperation(operation: ArrayBuffer): Promise<InsertResult> {
    return this.backend.insertOperation(operation);
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
