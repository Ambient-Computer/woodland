import Dataset from '../dataset';
import MemoryBackend from '../backend/memory';

describe('Dataset', () => {
  async function setup() {
    return await Dataset.create({
      backend: new MemoryBackend(),
    });
  }

  describe('create', () => {
    it('initializes a dataset', async () => {
      await expect(setup()).resolves.not.toThrow();
    });
  });
});
