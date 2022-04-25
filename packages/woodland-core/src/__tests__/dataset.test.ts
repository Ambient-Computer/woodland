import Dataset from '../dataset';
import MemoryBackend from '../backend/memory';
import { HistoricalRecordType } from '../types';

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

      const { operationId } = await dataset.insertOperation(
        new Uint8Array([8]),
      );

      await expect(backend.readOperation(operationId)).resolves.toEqual(
        new Uint8Array([8]),
      );
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
});
