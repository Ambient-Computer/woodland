import { CID } from 'multiformats/cid';
import { sha256 } from 'multiformats/hashes/sha2';
import createHash from '../hash';

describe('hash()', () => {
  it('produces a sha-256 CID', async () => {
    const cid = await createHash(new Uint8Array([1, 2, 3]));
    expect(cid.toString()).toMatchInlineSnapshot(
      `"bafkreiadsbmmn4waznesyuz3bjgrj33xzqhxrk6mz3ksq7meugrachh3qe"`,
    );

    // Sanity check: the generated hash should preserve the hash type.
    const decoded = CID.parse(cid.toString());
    expect(decoded.multihash.code).toBe(sha256.code);
  });
});
