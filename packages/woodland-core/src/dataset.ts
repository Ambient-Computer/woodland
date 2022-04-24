import type { StorageBackend } from './backend/interface';
import createHash, { type CID } from './hash';

export default class Dataset {
  private backend: StorageBackend;

  static async create(config: Config): Promise<Dataset> {
    return new Dataset(config);
  }

  private constructor(config: Config) {
    this.backend = config.backend;
  }

  async insertOperation(value: ArrayBuffer): Promise<InsertResult> {
    const cid = await createHash(value);
    const tsc = this.backend.createTransaction();
    tsc.addOperation(cid, value);

    await tsc.commit();

    return { id: cid };
  }
}

interface Config {
  /** A storage plugin. */
  backend: StorageBackend;
}

interface InsertResult {
  id: CID;
}
