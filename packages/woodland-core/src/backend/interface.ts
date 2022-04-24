import type { CID } from 'multiformats/cid';

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
}

export interface TransactionBuilder {
  /** Idempotently add the operation to the operations table. */
  addOperation(id: CID, operation: ArrayBuffer): void;

  /** Apply all changes simultaneously. */
  commit(): Promise<void>;
}
