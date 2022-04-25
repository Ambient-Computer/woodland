import { v4 as uuid } from 'uuid';
import MemoryStoragePlugin from '../memory';
import { createHash } from '../../utils';
import { HistoricalRecordType, TimelineRecord } from '../../types';

const datasetId = uuid({}, new Uint8Array(16));

describe('MemoryBackend', () => {
  describe('addOperation', () => {
    it('stores the new operation', async () => {
      const plugin = new MemoryStoragePlugin();
      const backend = await plugin.open(datasetId);
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
  });

  describe('appendToHistory', () => {
    it('stores the record sequentially', async () => {
      const plugin = new MemoryStoragePlugin();
      const backend = await plugin.open(datasetId);
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

  describe('addTimelineRecord', () => {
    it('stores the new timeline record', async () => {
      const plugin = new MemoryStoragePlugin();
      const backend = await plugin.open(datasetId);
      const tsc = backend.createTransaction();

      const genesis: TimelineRecord = { id: new Uint8Array([1, 2, 3]) };
      const cid = await createHash(genesis.id);

      // Pending, but not applied.
      tsc.addTimelineRecord(cid, genesis);
      await expect(backend.readTimelineRecord(cid)).resolves.toBeNull();

      await tsc.commit();
      await expect(backend.readTimelineRecord(cid)).resolves.toEqual(genesis);
    });
  });
});
