import MemoryBackend from '../memory';

describe('MemoryBackend', () => {
  it('implements the basic interface', async () => {
    const backend = new MemoryBackend();

    const tsc = backend.createTransaction();
    await tsc.commit();

    // TODO: This is a useless test. Replace it as the API takes shape.
  });
});
