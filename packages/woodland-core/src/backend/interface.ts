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
   * Transactions are automatically executed serially and allow multiple
   * instances of the same dataset to be open simultaneously (e.g. multiple tabs
   * on a webpage).
   */
  createTransaction<T>(runner: TransactionRunner<T>): Promise<T>;

  /** Close the storage backend. */
  close(): Promise<void>;
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

  /** ID must be an incrementing number. `0` is the first record. */
  put(id: number, record: HistoricalRecord): Promise<void>;
  get(id: number): Promise<HistoricalRecord | null>;

  /**
   * UNSAFE: Mutates the first record by generating a genesis block. This
   * should only be used to initialize local state.
   *
   * The transaction ID should be a hash of the dataset ID.
   */
  initialize(genesisId: CID): Promise<HistoricalRecord>;
}

export type { Timeline } from './timeline';
