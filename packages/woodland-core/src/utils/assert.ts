export default function assert(value: unknown, msg: string): asserts value {
  if (!value) {
    throw new AssertionError(msg);
  }
}

class AssertionError extends Error {
  name = 'AssertionError';
}
