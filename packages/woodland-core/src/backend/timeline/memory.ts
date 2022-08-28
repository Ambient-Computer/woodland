import type { CID } from 'multiformats/cid';
import type { Timeline } from './interface';

export default class MemoryTimeline implements Timeline {
  static withEntries(transactionIds: Array<CID>) {
    return new MemoryTimeline(transactionIds);
  }

  private constructor(private transactionIds: Array<CID>) {}

  fork(origin: number): MemoryTimeline {
    return new MemoryTimeline(this.transactionIds.slice(0, origin));
  }

  append(transactionIds: Array<CID>): MemoryTimeline {
    return new MemoryTimeline(this.transactionIds.concat(transactionIds));
  }

  async read(startingIndex: number, count: number): Promise<Array<CID>> {
    return this.transactionIds.slice(startingIndex, startingIndex + count);
  }
}
