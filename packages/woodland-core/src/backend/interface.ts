import type { CID } from 'multiformats/cid';
import type { HistoricalRecord, Transaction } from '../types';

/**
 * Storage plugins provide a facade over several databases:
 *
 * - A content-addressed KV store of operations
 * - A content-addressed KV store of transactions
 * - A transaction-addressed KV store of periodic state snapshots
 * - An ordered list of changes supporting the audit log
 *
 * Space-efficient use of the underlying engine requires some semantic
 * understanding of its structure, which is inherently coupled to Woodland's
 * feature set. For this reason, the interface is INTERNAL ONLY until the
 * implementation stabilizes.
 */
export interface StoragePlugin {
  /**
   * Each dataset gets a separate namespace. It doesn't need to be implemented
   * this way, but it definitely simplifies garbage collection when you can drop
   * entire databases.
   *
   * Note: the ID is a v4 UUID byte array.
   */
  open(datasetId: Uint8Array): Promise<StorageBackend>;
}

export interface StorageBackend {
  /**
   * Group operations into a single transaction. Transactions aren't strictly
   * necessary, but are preferred when available.
   */
  createTransaction(): TransactionBuilder;

  /** Return the operation according to its CID. */
  readOperation(id: CID): Promise<null | ArrayBuffer>;

  /** Return the transaction according to its CID. */
  readTransaction(id: CID): Promise<null | Transaction>;

  /** Returns the last record added to the changelog. */
  getMostRecentEvent(): Promise<HistoricalRecord>;

  createWritableTransaction<T>(runner: TransactionRunner<T>): Promise<T>;
}

export interface TransactionRunner<ReturnType> {
  (stores: ObjectStores): Promise<ReturnType>;
}

export interface ObjectStores {
  /** Grow-only set of content addressed operations. */
  operations: ObjectStore<ArrayBuffer>;

  /** Grow-only set of transactions linking operations together. */
  transactions: ObjectStore<Transaction>;

  /** Local log of changes. */
  history: HistoryStore;
}

export interface ObjectStore<Value> {
  /** Read a value. */
  get(id: CID): Promise<Value | null>;

  /** Write or replace a value. */
  put(id: CID, value: Value): Promise<void>;
}

export interface HistoryStore {
  append(record: HistoricalRecord): Promise<void>;
  getMostRecentEvent(): Promise<HistoricalRecord>;
}

export interface TransactionBuilder {
  /** Idempotently add the operation to the operations database. */
  addOperation(id: CID, operation: ArrayBuffer): TransactionBuilder;

  /** Idempotently add the record to the transaction database. */
  addTransaction(id: CID, record: Transaction): TransactionBuilder;

  /** Record a change. */
  appendToHistory(record: HistoricalRecord): TransactionBuilder;

  /** Apply all changes simultaneously. */
  commit(): Promise<void>;
}
