import { v4 as uuid } from 'uuid';
import Dataset from '../dataset';
import MemoryBackend from '../backend/memory';
import { HistoricalRecordType } from '../types';
import type { Message } from '../replication/interface';
import { quit } from '../replication/utils';

describe('Dataset', () => {
  async function setup() {
    const storage = new MemoryBackend();
    const dataset = await Dataset.create({ storage });
    return {
      backend: await storage.open(dataset.id),
      dataset,
      storage,
    };
  }

  describe('create', () => {
    it('initializes a dataset', async () => {
      const { backend } = await setup();

      await backend.createTransaction(async (stores) => {
        await expect(
          stores.history.getMostRecentEvent(),
        ).resolves.toBeDefined();
      });
    });
  });

  describe('open', () => {
    it('rejects if the dataset does not exist', async () => {
      const storage = new MemoryBackend();
      const id = uuid({}, new Uint8Array(16));
      const promise = Dataset.open(id, { storage });

      await expect(promise).rejects.toThrow(/Dataset does not exist/);
    });

    it('loads existing datasets', async () => {
      const { storage, dataset } = await setup();

      const promise = Dataset.open(dataset.id, { storage });
      await expect(promise).resolves.not.toThrow();
    });
  });

  describe('insertOperation', () => {
    it('saves the new operation in the storage backend', async () => {
      const { backend, dataset } = await setup();

      const { operationId } = await dataset.insertOperation(
        new Uint8Array([8]),
      );

      await backend.createTransaction(async (stores) => {
        await expect(stores.operations.get(operationId)).resolves.toEqual(
          new Uint8Array([8]),
        );
      });
    });

    it('generates a new transaction', async () => {
      const { backend, dataset } = await setup();

      const { transactionId, operationId } = await dataset.insertOperation(
        new Uint8Array(3),
      );

      await backend.createTransaction(async (stores) => {
        await expect(
          stores.transactions.get(transactionId),
        ).resolves.toMatchObject({
          operationId,
          index: 1,
        });
      });
    });

    it('appends to the changelog', async () => {
      const { dataset } = await setup();

      const { historicalRecord } = await dataset.insertOperation(
        new Uint8Array(),
      );

      expect(historicalRecord).toMatchObject({
        type: HistoricalRecordType.LocalChange,
        // TODO: Verify TL record CIDs once this is implemented.
      });
    });
  });

  describe('replication', () => {
    it('does nothing when given the `Quit` command', async () => {
      const { dataset } = await setup();

      const promise = dataset.execute(quit());

      await expect(promise).resolves.toEqual<Message>({
        reply: quit(),
        broadcast: quit(),
      });
    });

    it('does not fall to prototype trickery', async () => {
      const { dataset } = await setup();

      const promise = dataset.execute({
        type: 'toString' as any,
        payload: undefined,
      });

      await expect(promise).resolves.toEqual<Message>({
        reply: quit(),
        broadcast: quit(),
      });
    });
  });
});
