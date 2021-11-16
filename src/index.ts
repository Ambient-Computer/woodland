import { v4 as uuid } from 'uuid';

export class OperationLog<T, S extends Schema<any>> {
  private root: Operation<void> = {
    type: 'noop',
    children: [],
    prev: null,
    payload: undefined,
    id: '@@ROOT',
  };

  private graph: Map<string, Operation<T | void>> = new Map([
    [this.root.id, this.root],
  ]);

  private actions: Record<string, (a: any) => (prev: PrevId) => Operation<any>>;
  private head: string = this.root.id;
  views;

  constructor(config: Config<S>) {
    this.actions = Array.from(config.schema.types.entries()).reduce(
      (actions: Record<string, any>, [name, type]) => {
        actions[name] = (payload: any) => (prev: PrevId) => {
          const { matches, error } = type(payload);
          if (!matches) throw new Error(error);
          return {
            id: uuid(),
            type: name,
            prev,
            payload,
            children: [],
          };
        };

        return actions;
      },
      {},
    );

    this.views = Object.entries(config.views ?? {}).reduce(
      (views, [name, reducer]) => {
        let states = new WeakMap();
        views.set(name, {
          getState: (op = this.graph.get(this.head)) => states.get(op),
          setState: (op, value: any) => {
            states.set(op, value);
          },
          reducer,
        });

        return views;
      },
      new Map(),
    );
  }

  append(
    thunk: (
      actions: typeof this.actions,
    ) => Array<(prev: PrevId) => Operation<T>>,
  ): Delta<T> {
    const operations: Array<Operation<T>> = thunk(this.actions).reduce(
      (operations, thunk) => {
        const op = thunk(this.head) as Operation<T>;
        this.head = op.id;
        return operations.concat(op);
      },
      [],
    );

    operations.forEach((op) => {
      this.insert(op);
    });

    return { operations: new Map(operations.map((op) => [op.id, op])) };
  }

  integrate(delta: Delta<T>) {
    for (const op of delta.operations.values()) {
      if (this.graph.has(op.id)) return;
      this.insert(op);

      // HACK: I should be doing something more clever to determine the latest
      // element.
      this.head = op.id;
    }

    this.regenerateState(this.root);
  }

  private insert(op: Operation<T>) {
    const parent = this.graph.get(op.prev as string);
    parent!.children.push(op);
    parent!.children.sort(sortById);
    this.graph.set(op.id, op);

    this.regenerateState(op);
  }

  private regenerateState(startingPoint: Operation<T>) {
    const parent = this.graph.get(startingPoint.prev as string);

    this.recurse(startingPoint, (op) => {
      for (const view of this.views.values()) {
        const oldState = view.getState(parent);
        const newState = view.reducer(oldState, op);
        view.setState(op, newState);
      }
    });
  }

  private recurse(startingPoint: Operation<T>, fn: (op: Operation<T>) => any) {
    fn(startingPoint);
    startingPoint.children.forEach((op) => fn(op));
  }
}

export class Schema<
  Thunk extends (types: Validators) => Record<string, Validator>,
> {
  types: Map<string, Validator>;

  static define<Thunk extends (types: Validators) => Record<string, Validator>>(
    thunk: Thunk,
  ) {
    return new Schema(thunk);
  }

  private constructor(thunk: Thunk) {
    const types = thunk(validators);
    this.types = Object.entries(types).reduce((types, [name, type]) => {
      types.set(name, type);
      return types;
    }, new Map());
  }
}

const validators = {
  string: <T>(value: T) => ({
    matches: typeof value === 'string',
    error: `${value} is not a string.`,
  }),
  number: <T>(value: T) => ({
    matches: typeof value === 'number',
    error: `${value} is not a number.`,
  }),
  boolean: <T>(value: T) => ({
    matches: typeof value === 'boolean',
    error: `${value} is not a boolean.`,
  }),
  optional:
    (type: Validator) =>
    <T>(value: T) => {
      if (value === undefined) {
        return {
          matches: true,
          error: '',
        };
      }

      return type(value);
    },
  struct:
    <T extends Record<string, Validator>>(def: T) =>
    <V>(val: V) => {
      const isObject = Boolean(val) && typeof val === 'object';

      if (!val || typeof val !== 'object') {
        return {
          matches: false,
          error: `struct(...) requires an object.`,
        };
      }

      const mismatch = Object.entries(def).reduce((a, [k, type]) => {
        if (a) return a;

        const validation = type(val[k]);
        if (!validation.matches) return validation;

        return a;
      }, null);

      return {
        matches: mismatch === null,
        error: mismatch?.error ?? 'OH NO',
      };
    },
};

type Validators = typeof validators;
type Validator = Validators[keyof Validators];

type PrevId = string | null;

export interface Operation<T> {
  /** GUID */
  id: string;
  /** Previous ID */
  prev: PrevId;
  /** Linked list pointer going the other direction. */
  children: Array<Operation<T>>;
  /** Unique identifier referencing the schema. */
  type: string;
  /** The actual data of the operation. */
  payload: T;
}

interface Delta<T> {
  operations: Map<string, Operation<T>>;
}

interface Config<S extends Schema<any>> {
  schema: S;
  views: Record<string, (state: any, op: Operation<T>) => any>;
}

function sortById(a: Operation<unknown>, b: Operation<unknown>): number {
  return a.id < b.id ? -1 : 1;
}
