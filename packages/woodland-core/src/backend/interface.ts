import type { CID } from 'multiformats/cid';
import type { HistoricalRecord, TimelineRecord } from '../types';

/**
 * Storage plugins provide a facade over several databases:
 *
 * - A content-addressed KV store of operations
 * - A content-addressed KV store of timelines
 * - A timeline-addressed KV store of periodic state snapshots
 * - An ordered list of changes supporting the audit log
 *
 * Space-efficient use of the underlying engine requires some semantic
 * understanding of its structure, which is inherently coupled to Woodland's
 * feature set. For this reason, the interface is INTERNAL ONLY until the
 * implementation stabilizes.
 */
export interface StorageBackend {
  /**
   * Group operations into a single transaction. Transactions aren't strictly
   * necessary, but are preferred when available.
   */
  createTransaction(): TransactionBuilder;

  /** Return the operation according to its CID. */
  readOperation(id: CID): Promise<null | ArrayBuffer>;

  /** Return the timeline record according to its CID. */
  readTimelineRecord(id: CID): Promise<null | TimelineRecord>;

  /** Returns the last record added to the changelog. */
  getMostRecentEvent(): Promise<HistoricalRecord>;
}

export interface TransactionBuilder {
  /** Idempotently add the operation to the operations database. */
  addOperation(id: CID, operation: ArrayBuffer): TransactionBuilder;

  /** Idempotently add the record to the timeline database. */
  addTimelineRecord(id: CID, record: TimelineRecord): TransactionBuilder;

  /** Record a change. */
  appendToHistory(record: HistoricalRecord): TransactionBuilder;

  /** Apply all changes simultaneously. */
  commit(): Promise<void>;
}
