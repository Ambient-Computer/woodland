import MemoryBackend from '../memory';
import createHash from '../../hash';
import { HistoricalRecordType } from '../../types';

describe('MemoryBackend', () => {
  it('performs basic tasks correctly', async () => {
    const backend = new MemoryBackend();
    const tsc = backend.createTransaction();

    const operation = new Uint8Array([5]);
    const cid = await createHash(operation);

    // Pending, but not applied.
    tsc.addOperation(cid, operation);
    await expect(backend.readOperation(cid)).resolves.toBeNull();

    await tsc.commit();
    await expect(backend.readOperation(cid)).resolves.toEqual(
      new Uint8Array([5]),
    );
  });

  describe('appendToHistory', () => {
    it('stores the record sequentially', async () => {
      const backend = new MemoryBackend();
      const cid = await createHash(new Uint8Array());
      const record = {
        type: HistoricalRecordType.LocalChange,
        timestamp: new Date().toISOString(),
        changeset: [cid],
        timeline: cid,
      };

      await backend.createTransaction().appendToHistory(record).commit();

      await expect(backend.getMostRecentEvent()).resolves.toEqual(record);
    });
  });
});
