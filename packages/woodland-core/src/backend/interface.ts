export interface Backend {
  /** Group operations into a single transaction. */
  createTransaction(): Transaction;
}

export interface Transaction {
  /** Apply all changes simultaneously. */
  commit(): Promise<void>;
}
