import { encode } from 'multiformats/block';
import * as codec from 'multiformats/codecs/raw';
import { sha256 } from 'multiformats/hashes/sha2';
import type { CID } from 'multiformats/cid';

/**
 * Produces a sha-256 hash for arbitrary binary values. The multiformats
 * standard provides metadata so the hash algorithm can migrate to newer
 * implementations in the future.
 *
 * @todo: Move to utils/ folder.
 */
export default async function hash(value: ArrayBuffer): Promise<CID> {
  const block = await encode({
    hasher: sha256,
    codec,
    value,
  });

  return block.cid;
}

export type { CID };
