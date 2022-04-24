import Dataset from '../dataset';
import MemoryBackend from '../backend/memory';

describe('Dataset', () => {
  async function setup() {
    const backend = new MemoryBackend();
    const dataset = await Dataset.create({ backend });
    return {
      backend,
      dataset,
    };
  }

  describe('create', () => {
    it('initializes a dataset', async () => {
      await expect(setup()).resolves.not.toThrow();
    });
  });

  describe('insertOperation', () => {
    it('saves the new operation in the storage backend', async () => {
      const { backend, dataset } = await setup();

      const { id } = await dataset.insertOperation(new Uint8Array([8]));

      await expect(backend.readOperation(id)).resolves.toEqual(
        new Uint8Array([8]),
      );
    });
  });
});
