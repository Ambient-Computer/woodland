import type { CID } from 'multiformats/cid';

/* ===== LOCAL HISTORY ===== */

export interface HistoricalRecord {
  /** Identifies the type of change that happened. */
  type: HistoricalRecordType;

  /** ISO-8601 when the event occurred. Informational only. */
  timestamp: string;

  /** A list of newly-integrated timeline record IDs. */
  changeset: Array<CID>;

  /** The newest timeline as of this change. */
  timeline: CID;
}

export enum HistoricalRecordType {
  /** We received updates from other actors. */
  RemoteChange = 0,

  /** An operation was created locally. */
  LocalChange = 1,
}

/* ===== TIMELINES ===== */

/**
 * This has nothing to do with blockchains, it's just a very apt name. This is
 * the root element of every timeline in the dataset. The record's hash is
 * generated from it's UUID.
 */
export interface GenesisBlock {
  /** v4 UUID uniquely identifying the dataset. */
  id: Uint8Array;
}

/**
 * A timeline record links one operation to another forming an event log.
 * Timelines can fork under concurrent edits and sibling branches are ordered
 * deterministically. Each timeline is identified by the hash of it's previous
 * element (causal predecessor or parent) plus its operation ID:
 *
 *   id = hash((predecessorId || parentId) + this.operationId)
 *
 * This means the entire timeline is content-addressed, preventing cycles and
 * preserving edit history, as integrating older operations necessitates
 * immutably regenerating the records that follow.
 */
export interface TimelineBlock {
  /** The latest timeline record known while generating this one. */
  parentId: CID;

  /** The concurrent edit which preceeded this one (added on regeneration). */
  predecessorId: null | CID;

  /** The newly added operation. */
  operationId: CID;

  /** An incrementing number. Useful for reporting and periodic snapshots. */
  index: number;
}

export type TimelineRecord = GenesisBlock | TimelineBlock;
