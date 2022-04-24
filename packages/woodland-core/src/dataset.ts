import type { Backend } from './backend/interface';

export default class Dataset {
  static async create(config: Config): Promise<Dataset> {
    return new Dataset(config);
  }

  private constructor(_config: Config) {}
}

interface Config {
  /** A storage plugin. */
  backend: Backend;
}
