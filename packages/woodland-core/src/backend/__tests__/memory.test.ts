import { v4 as uuid } from 'uuid';
import MemoryStoragePlugin from '../memory';
import { createHash } from '../../utils';
import type { Transaction } from '../../types';
import { HistoricalRecordType } from '../../types';

const datasetId = uuid({}, new Uint8Array(16));

describe('MemoryBackend', () => {
  async function setup() {
    const plugin = new MemoryStoragePlugin();
    const id = uuid({}, new Uint8Array(16));

    return {
      backend: await plugin.open(id),
      id,
    };
  }

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
        head: cid,
      };

      await backend.createTransaction().appendToHistory(record).commit();

      await expect(backend.getMostRecentEvent()).resolves.toEqual(record);
    });
  });

  describe('addTransaction', () => {
    it('stores the new transaction', async () => {
      const plugin = new MemoryStoragePlugin();
      const backend = await plugin.open(datasetId);
      const tsc = backend.createTransaction();

      const genesis: Transaction = { id: new Uint8Array([1, 2, 3]) };
      const cid = await createHash(genesis.id);

      // Pending, but not applied.
      tsc.addTransaction(cid, genesis);
      await expect(backend.readTransaction(cid)).resolves.toBeNull();

      await tsc.commit();
      await expect(backend.readTransaction(cid)).resolves.toEqual(genesis);
    });
  });

  it('returns the result from the transaction', async () => {
    const { backend } = await setup();

    const message = 'transaction result';
    const result = await backend.createWritableTransaction(async () => {
      return message;
    });

    expect(result).toBe(message);
  });

  it('returns cached objects not yet committed', async () => {
    const { backend } = await setup();
    const id = await createHash(new Uint8Array());

    await backend.createWritableTransaction(async (stores) => {
      const operation = new Uint8Array();
      stores.operations.put(id, operation);
      await expect(stores.operations.get(id)).resolves.toEqual(operation);
    });
  });

  it('returns objects already written in the store', async () => {
    const { backend } = await setup();
    const id = await createHash(new Uint8Array());
    const operation = new Uint8Array();

    await backend.createWritableTransaction(async (stores) => {
      stores.operations.put(id, operation);
    });

    await backend.createWritableTransaction(async (stores) => {
      await expect(stores.operations.get(id)).resolves.toEqual(operation);
    });
  });

  it('can optimistically append items to the changelog', async () => {
    const { backend } = await setup();

    await backend.createWritableTransaction(async (stores) => {
      const id = await createHash(new Uint8Array());
      const record = {
        type: HistoricalRecordType.LocalChange,
        timestamp: new Date().toISOString(),
        head: id,
        changeset: [id],
      };

      stores.history.append(record);

      await expect(stores.history.getMostRecentEvent()).resolves.toEqual(
        record,
      );
    });
  });

  it('returns committed records in the transaction', async () => {
    const { backend } = await setup();
    const id = await createHash(new Uint8Array());
    const record = {
      type: HistoricalRecordType.LocalChange,
      timestamp: new Date().toISOString(),
      head: id,
      changeset: [id],
    };

    await backend.createWritableTransaction(async (stores) => {
      stores.history.append(record);
    });

    await backend.createWritableTransaction(async (stores) => {
      await expect(stores.history.getMostRecentEvent()).resolves.toEqual(
        record,
      );
    });
  });
});
