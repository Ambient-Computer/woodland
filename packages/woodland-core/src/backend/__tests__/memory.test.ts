import { v4 as uuid } from 'uuid';
import MemoryStoragePlugin from '../memory';
import { createHash } from '../../utils';
import { HistoricalRecordType } from '../../types';

describe('MemoryBackend', () => {
  async function setup() {
    const plugin = new MemoryStoragePlugin();
    const id = uuid({}, new Uint8Array(16));

    return {
      backend: await plugin.open(id),
      id,
    };
  }

  it('returns the result from the transaction', async () => {
    const { backend } = await setup();

    const message = 'transaction result';
    const result = await backend.createTransaction(async () => {
      return message;
    });

    expect(result).toBe(message);
  });

  it('returns cached objects not yet committed', async () => {
    const { backend } = await setup();
    const id = await createHash(new Uint8Array());

    await backend.createTransaction(async (stores) => {
      const operation = new Uint8Array();
      stores.operations.put(id, operation);
      await expect(stores.operations.get(id)).resolves.toEqual(operation);
    });
  });

  it('returns objects already written in the store', async () => {
    const { backend } = await setup();
    const id = await createHash(new Uint8Array());
    const operation = new Uint8Array();

    await backend.createTransaction(async (stores) => {
      stores.operations.put(id, operation);
    });

    await backend.createTransaction(async (stores) => {
      await expect(stores.operations.get(id)).resolves.toEqual(operation);
    });
  });

  it('can optimistically append items to the changelog', async () => {
    const { backend } = await setup();

    await backend.createTransaction(async (stores) => {
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

    await backend.createTransaction(async (stores) => {
      stores.history.append(record);
    });

    await backend.createTransaction(async (stores) => {
      await expect(stores.history.getMostRecentEvent()).resolves.toEqual(
        record,
      );
    });
  });

  it('allows insertion/retrieval of history at any index', async () => {
    const { backend } = await setup();
    const id = await createHash(new Uint8Array());
    const record = {
      type: HistoricalRecordType.LocalChange,
      timestamp: new Date().toISOString(),
      head: id,
      changeset: [id],
    };

    await backend.createTransaction(async (stores) => {
      await stores.history.put(0, record);
    });

    await backend.createTransaction(async (stores) => {
      await expect(stores.history.get(0)).resolves.toBe(record);
      await expect(stores.history.getMostRecentEvent()).resolves.toBe(record);
    });
  });
});
