// A fake `loams:server` for the Live app's unit tests (FT0 Task 9): the function
// constructors, the validators the bundle uses (checked as strictly as Live checks them:
// `v.int64()` takes only a BigInt), and an in-memory `ctx.db` with the index reads the
// bundle uses. The real runtime is `crates/loams-live-js`.

type Validator = (value: unknown, path: string) => void;
type Doc = Record<string, unknown> & { _id: string };

export interface FunctionDef {
  kind: 'query' | 'mutation';
  args: Record<string, Validator>;
  handler: (ctx: { db: FakeDb }, args: Record<string, unknown>) => Promise<unknown>;
}

const define =
  (kind: FunctionDef['kind']) =>
  (def: { args?: Record<string, Validator>; handler: FunctionDef['handler'] }): FunctionDef => ({
    kind,
    args: def.args ?? {},
    handler: def.handler,
  });

export const query = define('query');
export const mutation = define('mutation');

const OPTIONAL = Symbol('optional');
type Optional = Validator & { [OPTIONAL]: true };

export const v = {
  string: (): Validator => (x, p) => {
    if (typeof x !== 'string') throw new TypeError(`${p} is a string`);
  },
  int64: (): Validator => (x, p) => {
    if (typeof x !== 'bigint') throw new TypeError(`${p} is an int64 (a BigInt)`);
  },
  any: (): Validator => () => {},
  optional: (inner: Validator): Optional => {
    const f = ((x: unknown, p: string) => {
      if (x !== undefined) inner(x, p);
    }) as Optional;
    f[OPTIONAL] = true;
    return f;
  },
};

class Query {
  #filter: [string, unknown][] = [];
  #sort: string[] = [];
  #order: 'asc' | 'desc' = 'asc';
  constructor(
    private readonly docs: Doc[],
    private readonly indexes: Record<string, string[]>,
  ) {}
  withIndex(name: string, build?: (q: { eq: (f: string, v: unknown) => unknown }) => unknown) {
    const fields = this.indexes[name];
    if (!fields) throw new Error(`no index ${name}`);
    this.#sort = fields;
    const q = {
      eq: (field: string, value: unknown) => {
        this.#filter.push([field, value]);
        return q;
      },
    };
    build?.(q);
    return this;
  }
  order(order: 'asc' | 'desc') {
    this.#order = order;
    return this;
  }
  #rows(): Doc[] {
    const rows = this.docs.filter((d) => this.#filter.every(([f, value]) => d[f] === value));
    const key = (d: Doc) => this.#sort.map((f) => d[f] as number | string);
    rows.sort((a, b) => {
      const [x, y] = [key(a), key(b)];
      for (let i = 0; i < x.length; i++) {
        const [xi, yi] = [x[i] as number | string, y[i] as number | string];
        if (xi !== yi) return xi < yi ? -1 : 1;
      }
      return 0;
    });
    return this.#order === 'desc' ? rows.reverse() : rows;
  }
  async first() {
    return structuredClone(this.#rows()[0] ?? null);
  }
  async take(n: number) {
    return structuredClone(this.#rows().slice(0, n));
  }
  async collect() {
    return structuredClone(this.#rows());
  }
}

/** One app's documents, with the schema's indexes. */
export class FakeDb {
  tables = new Map<string, Doc[]>();
  #next = 0;
  constructor(private readonly indexes: Record<string, Record<string, string[]>>) {}
  #table(name: string) {
    let rows = this.tables.get(name);
    if (!rows) {
      rows = [];
      this.tables.set(name, rows);
    }
    return rows;
  }
  query(table: string) {
    return new Query(this.#table(table), this.indexes[table] ?? {});
  }
  async insert(table: string, doc: Record<string, unknown>) {
    const _id = `${table}:${++this.#next}`;
    this.#table(table).push({ ...structuredClone(doc), _id });
    return _id;
  }
  async patch(id: string, fields: Record<string, unknown>) {
    for (const rows of this.tables.values()) {
      const doc = rows.find((d) => d._id === id);
      if (doc) {
        for (const [k, value] of Object.entries(fields)) {
          if (value === undefined) throw new Error(`ctx.db.patch: field "${k}" is undefined`);
          doc[k] = structuredClone(value);
        }
        return;
      }
    }
    throw new Error(`no document ${id}`);
  }
  async get(id: string) {
    for (const rows of this.tables.values()) {
      const doc = rows.find((d) => d._id === id);
      if (doc) return structuredClone(doc);
    }
    return null;
  }
}

/** Call one function as the host does: validate the arguments, then run it. */
export async function call(def: FunctionDef, db: FakeDb, args: Record<string, unknown>) {
  for (const key of Object.keys(args)) {
    if (!(key in def.args)) throw new TypeError(`unexpected argument ${key}`);
  }
  for (const [key, check] of Object.entries(def.args)) check(args[key], key);
  return await def.handler({ db }, args);
}
