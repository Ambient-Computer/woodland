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
