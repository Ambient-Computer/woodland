import { Instruction, InstructionType } from './interface';

export function quit(): Instruction<InstructionType.Quit> {
  return {
    type: InstructionType.Quit,
    payload: undefined,
  };
}
