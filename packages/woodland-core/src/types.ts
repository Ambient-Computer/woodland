import type { CID } from 'multiformats/cid';
import type { Timeline } from './backend/timeline';

/* ===== LOCAL HISTORY ===== */

export interface HistoricalRecord {
  /** Identifies the type of change that happened. */
  type: HistoricalRecordType;

  /** ISO-8601 when the event occurred. Informational only. */
  timestamp: string;

  /** A list of newly-integrated transaction IDs. */
  changeset: Array<CID>;

  /** The newest transaction as of this change. */
  head: CID;

  /** An index of the sequence of transactions. Derived from the oplog. */
  timeline: Timeline;
}

export enum HistoricalRecordType {
  /** We received updates from other actors. */
  RemoteChange = 0,

  /** An operation was created locally. */
  LocalChange = 1,
}

/* ===== TRANSACTIONS ===== */

/**
 * This has nothing to do with blockchains, it's just a very apt name. This is
 * the root element of every transaction in the dataset. The record's hash is
 * generated from it's UUID.
 */
export interface GenesisBlock {
  /** v4 UUID uniquely identifying the dataset. */
  id: Uint8Array;
}

/**
 * A transaction links one operation to another forming an event log.
 * Transactions can fork under concurrent edits and sibling branches are
 * ordered deterministically. Each transaction is identified by the hash of
 * it's previous element (causal predecessor or parent) plus its operation ID:
 *
 *   id = hash((predecessorId || parentId) + this.operationId)
 *
 * This means the entire transaction is content-addressed, preventing cycles
 * and preserving edit history, as integrating older operations necessitates
 * immutably regenerating the records that follow.
 */
export interface Record {
  /** The latest transaction known while generating this one. */
  parentId: CID;

  /** The concurrent edit which preceeded this one (added on regeneration). */
  predecessorId: null | CID;

  /** The newly added operation. */
  operationId: CID;

  /** An incrementing number. Useful for reporting and periodic snapshots. */
  index: number;
}

export type Transaction = GenesisBlock | Record;

/* ===== DELTAS ===== */

export interface InsertionDelta {
  /**
   * Latest checksum known to the dataset. Note: If we're paginating, the
   * transaction set might still be thousands of operations behind `head`.
   */
  head: CID;

  /**
   * An ordered list of transactions to integrate. If this is received from
   * a remote, double check the hash integrity before saving anything.
   */
  transactions: Array<Record>;

  /**
   * Operations indexed by their hash. The hash is convenient for search but
   * should be verified before saving anything.
   */
  operations: Map<CID, ArrayBuffer>;
}
