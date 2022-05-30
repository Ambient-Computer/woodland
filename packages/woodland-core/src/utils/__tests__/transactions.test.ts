import { hashIds, hashTransaction } from '../transactions';
import { createHash } from '../';

describe('Transaction utils', () => {
  describe('hashIds', () => {
    it('generates a consistent ID', async () => {
      const id1 = await createHash(new Uint8Array([0]));
      const id2 = await createHash(new Uint8Array([1]));
      await expect(hashIds([id1, id2])).resolves.toEqual(
        await hashIds([id1, id2]),
      );
    });
  });

  describe('hashTransaction', () => {
    it('hashes the transaction by parent + operation', async () => {
      const id = await hashTransaction({
        parentId: await createHash(new Uint8Array([0])),
        predecessorId: null,
        operationId: await createHash(new Uint8Array()),
        index: 1,
      });

      expect(id.toString()).toMatchInlineSnapshot(
        `"bafkreid6yj4zqeftrycbwbdpipchuh3bfljcrnfthrwhmzfwb4j2zczzui"`,
      );
    });

    it('prefers the predecessor ID over the parent', async () => {
      const id = await hashTransaction({
        parentId: await createHash(new Uint8Array([0])),
        predecessorId: await createHash(new Uint8Array([1])),
        operationId: await createHash(new Uint8Array()),
        index: 1,
      });

      expect(id.toString()).toMatchInlineSnapshot(
        `"bafkreigxh7jsueikjsybqwbhnjqirjwewxqgtmhxgmjziduawjblpjydxa"`,
      );
    });
  });
});
