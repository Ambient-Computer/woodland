import type { CID } from 'multiformats/cid';
import type { Transaction, InsertionDelta } from '../types';

/**
 * States:
 *
 * - They are ahead of us (appended transactions)
 * - They are behind us (missing our transactions)
 * - They are beside us (we forked at some point)
 *
 * We need to specially consider two cases:
 *
 * 1. Those beside us may have forked thousands of operations ago
 * 2. Those behind us may be thousands of operations behind
 *
 * Both situations need a type of interactive long-running pagination.
 */

/**
 * Interacts with the network layer by indicating which updates need to be
 * forwarded, challenged, disregarded, and where the messages should be sent.
 *
 * NOTE: This would do better as an API. By removing assumptions of the
 * network topology, we make it work for most cases, but remove optimizations
 * from all cases. Bespoke adapters seem more appropriate.
 *
 * Consider a 3-peer daisy chain topology where we are in the middle. There are
 * two datasets, A and B, shared by all members except this peer. If an update
 * arrives from dataset B but we only carry dataset A, this algorithm would
 * ignore the update as irrelevant and accidentally partition the other peer.
 */
export interface Message {
  /** A reply to the actor that provided the new transactions. */
  reply: Instruction<InstructionType>;

  /** A broadcast for everyone else. */
  broadcast: Instruction<InstructionType>;
}

export interface Instruction<Type extends InstructionType> {
  type: Type;
  payload: Instructions[Type];
}

export enum InstructionType {
  /** The sequence is done. No need to send a reply. */
  Quit = 0,

  /** Send new transactions or send `HEAD` to initiate a handshake. */
  Integrate = 1,

  /**
   * We need common ancestors to merge. Search backward in exponentially
   * increasing slices.
   */
  FindCommonAncestor = 2,

  /** Results from `FindCommonAncestor`. */
  ReceiveAncestors = 3,
}

export interface Instructions {
  [InstructionType.Quit]: void;
  [InstructionType.Integrate]: InsertionDelta;
  [InstructionType.FindCommonAncestor]: {
    /** The transaction to start searching backwards from. */
    cursor: CID;

    /** The starting scan exponent (useful for resuming scans). */
    stepSize: number;
  };

  [InstructionType.ReceiveAncestors]: {
    /**
     * An ordered list of non-sequential transactions spaced by increasing
     * distance. Use this to find the closest common ancestor and start
     * syncing with `Integrate`.
     */
    transactions: Array<Transaction>;

    /**
     * The final exponential multiplier. Use this to resume searches or decide
     * if a refinement is necessary.
     */
    stepSize: number;
  };
}
