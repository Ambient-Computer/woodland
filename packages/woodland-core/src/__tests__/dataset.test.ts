import { v4 as uuid } from 'uuid';
import Dataset from '../dataset';
import MemoryBackend from '../backend/memory';
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

  describe('close', () => {
    it('closes the storage backend', async () => {
      const { dataset } = await setup();

      await dataset.close();
      const promise = dataset.insertOperation(new Uint8Array());

      await expect(promise).rejects.toThrow(/closed/i);
    });
  });

  describe('replication', () => {
    it('does nothing when given the `Quit` command', async () => {
      const { dataset } = await setup();

      const promise = dataset.execute(quit());
      const expected: Message = {
        reply: quit(),
        broadcast: quit(),
      };

      await expect(promise).resolves.toEqual(expected);
    });

    it('does not fall to prototype trickery', async () => {
      const { dataset } = await setup();

      const promise = dataset.execute({
        type: 'toString' as any,
        payload: undefined,
      });

      const expected: Message = {
        reply: quit(),
        broadcast: quit(),
      };

      await expect(promise).resolves.toEqual(expected);
    });
  });
});
