import type { Instruction } from './interface';
import { InstructionType } from './interface';

export function quit(): Instruction<InstructionType.Quit> {
  return {
    type: InstructionType.Quit,
    payload: undefined,
  };
}
