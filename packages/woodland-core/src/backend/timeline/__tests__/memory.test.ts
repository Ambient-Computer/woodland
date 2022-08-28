import { CID, createHash } from '../../../utils';
import Timeline from '../memory';

describe('MemoryTimeline', () => {
  async function setup(entries: Array<CID> = []) {
    return {
      timeline: Timeline.withEntries(entries),
    };
  }

  it('includes the initial records', async () => {
    const hash = await createHash(new Uint8Array([1, 2, 3]));
    const { timeline } = await setup([hash]);

    await expect(timeline.read(0, 10)).resolves.toEqual([hash]);
  });

  it('only reads the records you ask for', async () => {
    const [h1, h2, h3] = await Promise.all([
      createHash(new Uint8Array([1, 2, 3])),
      createHash(new Uint8Array([4, 5, 6])),
      createHash(new Uint8Array([7, 8, 9])),
    ]);

    const { timeline } = await setup([h1, h2, h3, h1]);

    await expect(timeline.read(1, 2)).resolves.toEqual([h2, h3]);
  });

  it('can append items', async () => {
    const { timeline: original } = await setup([]);
    const hash = await createHash(new Uint8Array());

    const update = original.append([hash]);

    await expect(original.read(0, 1)).resolves.toEqual([]);
    await expect(update.read(0, 1)).resolves.toEqual([hash]);
  });

  it('can fork from a point in history', async () => {
    const [h1, h2, h3] = await Promise.all([
      createHash(new Uint8Array([1, 2, 3])),
      createHash(new Uint8Array([4, 5, 6])),
      createHash(new Uint8Array([7, 8, 9])),
    ]);

    const { timeline } = await setup([h1, h2]);
    const fork = timeline.fork(1).append([h3]);

    await expect(fork.read(0, 10)).resolves.toEqual([h1, h3]);
  });
});
