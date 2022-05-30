import type { StoragePlugin } from './backend/interface';
import { Instruction, Message, InstructionType } from './replication/interface';
import type { InsertionDelta } from './types';
import BackendController from './backend-controller';
import { quit } from './replication/utils';

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
    const controller = await BackendController.openDataset({
      storage: config.storage,
      datasetId,
    });

    return new Dataset(datasetId, controller);
  }

  private constructor(
    /** A unique, persistent UUID which globally identifies the dataset. */
    public readonly id: Uint8Array,
    private backend: BackendController,
  ) {}

  /** Permanently close the dataset. */
  async close() {
    await this.backend.close();
  }

  async insertOperation(operation: ArrayBuffer): Promise<InsertionDelta> {
    return this.backend.insertOperation(operation);
  }

  /** Receive and process an instruction from a remote dataset. */
  async execute<Type extends InstructionType>(
    instruction: Instruction<Type>,
  ): Promise<Message> {
    if (instruction?.type && this.handlers.hasOwnProperty(instruction.type)) {
      const handler: InstructionHandler<Type> = this.handlers[instruction.type];
      return handler(instruction);
    }

    // Invalid message.
    return {
      reply: quit(),
      broadcast: quit(),
    };
  }

  private handlers = {
    [InstructionType.Quit]: () => ({
      reply: quit(),
      broadcast: quit(),
    }),
    [InstructionType.Integrate]: () => ({
      reply: quit(),
      broadcast: quit(),
    }),
    [InstructionType.FindCommonAncestor]: () => ({
      reply: quit(),
      broadcast: quit(),
    }),
    [InstructionType.ReceiveAncestors]: () => ({
      reply: quit(),
      broadcast: quit(),
    }),
  };
}

interface Config {
  /** A storage plugin. */
  storage: StoragePlugin;
}

interface InstructionHandler<Type extends InstructionType> {
  (instruction: Instruction<Type>): Message;
}
