import { v4 as uuid } from 'uuid';
import BackendController from '../backend-controller';
import MemoryStoragePlugin from '../backend/memory';
import { HistoricalRecordType } from '../types';

describe('BackendController', () => {
  async function setup() {
    const storage = new MemoryStoragePlugin();
    const { controller, genesis } = await BackendController.initializeDataset(
      storage,
    );

    return {
      backend: await storage.open(genesis.id),
      controller,
    };
  }

  describe('initializeDataset', () => {
    it('creates the dataset', async () => {
      const result = await BackendController.initializeDataset(
        new MemoryStoragePlugin(),
      );

      expect(result.controller).toBeInstanceOf(BackendController);
    });
  });

  describe('openDataset', () => {
    it('fails if the dataset does not exist', async () => {
      const datasetId = uuid({}, new Uint8Array(16));
      const promise = BackendController.openDataset({
        storage: new MemoryStoragePlugin(),
        datasetId,
      });

      await expect(promise).rejects.toThrow(/Dataset does not exist/);
    });

    it('loads existing datasets', async () => {
      const storage = new MemoryStoragePlugin();
      const { genesis } = await BackendController.initializeDataset(storage);

      const promise = BackendController.openDataset({
        datasetId: genesis.id,
        storage,
      });

      await expect(promise).resolves.toBeInstanceOf(BackendController);
    });
  });

  describe('insertOperation', () => {
    it('saves the new operation in the storage backend', async () => {
      const { backend, controller } = await setup();

      const { operations } = await controller.insertOperation(
        new Uint8Array([8]),
      );

      const [operationId] = operations.keys();

      await backend.createTransaction(async (stores) => {
        await expect(stores.operations.get(operationId)).resolves.toEqual(
          new Uint8Array([8]),
        );
      });
    });

    it('generates a new transaction', async () => {
      const { backend, controller } = await setup();

      const { operations } = await controller.insertOperation(
        new Uint8Array(3),
      );

      const [operationId] = operations.keys();
      await backend.createTransaction(async (stores) => {
        const { head, type } = await stores.history.getMostRecentEvent();
        expect(type).toBe(HistoricalRecordType.LocalChange);

        await expect(stores.transactions.get(head)).resolves.toMatchObject({
          operationId,
          index: 1,
        });
      });
    });
  });
});
