import type { Backend, Transaction } from './interface';

export default class MemoryBackend implements Backend {
  createTransaction() {
    return new TransactionBuilder();
  }
}

class TransactionBuilder implements Transaction {
  async commit() {
    return;
  }
}
