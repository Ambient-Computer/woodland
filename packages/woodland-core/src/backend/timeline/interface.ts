import type { CID } from 'multiformats/cid';

/**
 * A 2 dimensional index for timelines. Associates a transaction index number
 * with the corresponding record ID. Timelines are local-only. Every time you
 * retroactively integrate a remote transaction, the timeline forks at the
 * common ancestor.
 *
 * Timelines update using a builder pattern. Changes must be committed.
 */
export interface Timeline {
  /**
   * UNSAFE: Associate an index number with transaction. Make sure this
   * executes sequentially.
   */
  append(transactionIds: Array<CID>): Timeline;

  /**
   * Forks the timeline at a transaction index and creates a new timeline.
   * Make sure this executes sequentially. New updates could be lost otherwise.
   */
  fork(index: number): Timeline;

  /** Pull the records for a given range. */
  read(startingIndex: number, count: number): Promise<Array<CID>>;
}
