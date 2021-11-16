import { v4 as uuid } from 'uuid';
import * as woodland from '../index';

const views = {
  counter: (state = 0, op: woodland.Operation<any>) => {
    if (op.type === 'increment') {
      return state + op.payload;
    }

    if (op.type === 'decrement') {
      return state - op.payload;
    }

    return state;
  },
  users: (state = {}, op: woodland.Operation<any>) => {
    if (op.type === 'createUser') {
      const { id, firstName, lastName } = op.payload;
      return {
        ...state,
        [id]: { firstName, lastName },
      };
    }

    if (op.type === 'updateUser') {
      const { id, ...patch } = op.payload;
      return {
        ...state,
        [id]: { ...state[id], ...patch },
      };
    }

    return state;
  },
};

const schema = woodland.Schema.define((t) => ({
  increment: t.number,
  decrement: t.number,
  createUser: t.struct({
    firstName: t.string,
    lastName: t.string,
    id: t.string,
  }),
  updateUser: t.struct({
    firstName: t.optional(t.string),
    lastName: t.optional(t.string),
    id: t.string,
  }),
}));

const log = new woodland.OperationLog({
  schema,
  views,
});

const replica = new woodland.OperationLog({
  schema,
  views,
});

Object.assign(globalThis, woodland, {
  schema,
  log,
  replica,
});

const delta = log.append((actions) => [
  actions.increment(1),
  actions.increment(5),
  actions.decrement(10),
  actions.createUser({
    firstName: 'Delores',
    lastName: 'Abernathy',
    id: 'someId',
  }),
  actions.updateUser({
    id: 'someId',
    lastName: 'Wyatt',
  }),
]);

replica.integrate(delta);
