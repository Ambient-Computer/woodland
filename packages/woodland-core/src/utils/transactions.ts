import { createHash, CID } from './';
import type { Record } from '../types';

export function hashIds(values: Array<CID>): Promise<CID> {
  const byteLength = values.reduce(
    (byteLength, value) => byteLength + value.byteLength,
    0,
  );

  const src = new Uint8Array(byteLength);
  values.reduce((startingIndex: number, id: CID) => {
    src.set(id.bytes, startingIndex);
    return startingIndex + id.bytes.byteLength;
  }, 0);

  return createHash(src);
}

// hash(lastRecordId + newOperationId)
export async function hashTransaction(transaction: Record): Promise<CID> {
  const predecessor = transaction.predecessorId || transaction.parentId;
  return hashIds([predecessor, transaction.operationId]);
}
