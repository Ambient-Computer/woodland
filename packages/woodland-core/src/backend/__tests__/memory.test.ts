import MemoryBackend from '../memory';
import createHash from '../../hash';

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
});
