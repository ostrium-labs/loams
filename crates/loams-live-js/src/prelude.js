// The prelude of every Loams Live function context (LV1 plan Task 3; R1
// Task 13; design §20 §6.2). It runs before the bundle, in a fresh context,
// and returns the internals the host uses. In order it:
//
//   1. replaces the non-deterministic built-ins: `Date` reads the call's
//      start timestamp, `Math.random` draws from the host's seeded stream,
//      and every property of `crypto` throws `DeterminismError`;
//   2. defines `console` (collected per call by the host), guards the
//      Array methods QuickJS runs without interrupt checks, and defines the
//      `loams:server` exports (`query`, `mutation`, `internalQuery`,
//      `internalMutation`) with `ctx.db`;
//   3. deletes every global outside the host's allowlist (no timers, no
//      `fetch`, no `WebAssembly`, no `performance`, no `WeakRef`), makes the
//      rest read-only and deep-freezes every built-in, so the bundle cannot
//      undo step 1. The global object itself stays extensible: a bundle may
//      add globals of its own, which live only as long as its context. Assigning to an inherited `name`, `message`, `toString`,
//      `valueOf` or `constructor` still defines an own property, as it would
//      without the freeze.
//
// The script's value is `setup(natives, config)`, which returns the
// internals.
(function setup(natives, config) {
  "use strict";

  const { now, random, log, host } = natives;
  // `globalThis.Date`: the name `Date` below is the hoisted shim.
  const OriginalDate = globalThis.Date;
  const defineProperty = Object.defineProperty;
  const freeze = Object.freeze;
  const getPrototypeOf = Object.getPrototypeOf;
  const ownKeys = Reflect.ownKeys;
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  const apply = Reflect.apply;
  const construct = Reflect.construct;
  const keys = Object.keys;
  const isArray = Array.isArray;
  const stringify = JSON.stringify;
  const fromCharCode = String.fromCharCode;
  const join = Array.prototype.join;
  const method = (value) => ({ value, writable: true, enumerable: false, configurable: true });

  // ---- 1. Determinism ----

  class DeterminismError extends Error {}
  defineProperty(DeterminismError.prototype, "name", method("DeterminismError"));
  const NO_CRYPTO =
    "crypto randomness is not available in queries and mutations; use an action";

  function Date(...args) {
    if (new.target === undefined) {
      return new OriginalDate(now()).toString();
    }
    if (args.length === 0) {
      return construct(OriginalDate, [now()], new.target);
    }
    return construct(OriginalDate, args, new.target);
  }
  defineProperty(Date, "length", { value: 7 });
  defineProperty(Date, "prototype", { value: OriginalDate.prototype, writable: false });
  defineProperty(OriginalDate.prototype, "constructor", method(Date));
  defineProperty(Date, "now", method(function now_() {
    return now();
  }));
  defineProperty(Date.now, "name", { value: "now" });
  defineProperty(Date, "parse", method(OriginalDate.parse));
  defineProperty(Date, "UTC", method(OriginalDate.UTC));
  defineProperty(globalThis, "Date", method(Date));

  defineProperty(Math, "random", method(function random_() {
    return random();
  }));
  defineProperty(Math.random, "name", { value: "random" });

  const crypto = new Proxy({}, {
    get() {
      throw new DeterminismError(NO_CRYPTO);
    },
  });
  defineProperty(globalThis, "crypto", method(crypto));

  // ---- 2. console ----

  const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

  function inspect(value, depth, seen) {
    switch (typeof value) {
      case "string":
        return depth === 0 ? value : stringify(value);
      case "bigint":
        return `${value}n`;
      case "symbol":
        return value.toString();
      case "function":
        return `[Function: ${value.name || "(anonymous)"}]`;
      case "object":
        break;
      default:
        return String(value);
    }
    if (value === null) {
      return "null";
    }
    if (seen.includes(value)) {
      return "[Circular]";
    }
    if (value instanceof Error) {
      return `${value.name}: ${value.message}`;
    }
    if (value instanceof ArrayBuffer) {
      return `ArrayBuffer { byteLength: ${value.byteLength} }`;
    }
    if (depth > 3) {
      return isArray(value) ? "[Array]" : "[Object]";
    }
    const inner = [...seen, value];
    if (isArray(value)) {
      if (value.length === 0) {
        return "[]";
      }
      const parts = [];
      for (let i = 0; i < value.length && i < 100; i++) {
        parts.push(inspect(value[i], depth + 1, inner));
      }
      if (value.length > 100) {
        parts.push(`... ${value.length - 100} more items`);
      }
      return `[ ${parts.join(", ")} ]`;
    }
    const names = keys(value);
    if (names.length === 0) {
      return "{}";
    }
    const parts = names.slice(0, 100).map((k) => {
      const key = IDENT.test(k) ? k : stringify(k);
      return `${key}: ${inspect(value[k], depth + 1, inner)}`;
    });
    if (names.length > 100) {
      parts.push(`... ${names.length - 100} more keys`);
    }
    return `{ ${parts.join(", ")} }`;
  }

  function emit(level, args) {
    let line;
    try {
      line = args.map((a) => inspect(a, 0, [])).join(" ");
    } catch (e) {
      line = "[unprintable]";
    }
    if (line.length > config.lineBytes) {
      line = line.slice(0, config.lineBytes);
    }
    log(level, line);
  }

  const console = {};
  for (const level of ["debug", "log", "info", "warn", "error"]) {
    defineProperty(console, level, {
      value: { [level]: (...args) => emit(level, args) }[level],
      enumerable: true,
    });
  }
  defineProperty(globalThis, "console", method(console));

  // ---- 2. Array methods that QuickJS runs without interrupt checks ----

  // On an array-like longer than `config.maxLength`, these built-ins loop in
  // C over every index without polling the interrupt handler, so
  // `Array(2 ** 32 - 1).join()` would run for minutes past the CPU limit.
  // An array that long is sparse (a dense one would exceed the memory
  // limit), so they refuse it with a RangeError instead.
  const MAX_LENGTH = config.maxLength;
  const ArrayProto = Array.prototype;

  function guardLength(target, name) {
    if (target === null || target === undefined) {
      return;
    }
    const length = Number(target.length);
    if (length > MAX_LENGTH) {
      throw new RangeError(
        `Array.prototype.${name}: an array of length ${length} is longer than functions ` +
          `may process (${MAX_LENGTH}); an array this long is sparse`,
      );
    }
  }

  function guard(name, check) {
    const original = ArrayProto[name];
    const shim = {
      [name](...args) {
        check(this, args);
        return apply(original, this, args);
      },
    }[name];
    defineProperty(shim, "length", { value: original.length });
    defineProperty(ArrayProto, name, method(shim));
  }

  for (const name of [
    "join",
    "toLocaleString",
    "reverse",
    "slice",
    "splice",
    "shift",
    "unshift",
    "copyWithin",
    "sort",
  ]) {
    guard(name, (self) => guardLength(self, name));
  }
  guard("concat", (self, args) => {
    guardLength(self, "concat");
    for (const arg of args) {
      if (isArray(arg)) {
        guardLength(arg, "concat");
      }
    }
  });
  function scanFlat(target, depth) {
    guardLength(target, "flat");
    if (!(depth >= 1)) {
      return;
    }
    const length = Number(target.length);
    for (let i = 0; i < length; i++) {
      const item = target[i];
      if (isArray(item)) {
        scanFlat(item, depth - 1);
      }
    }
  }
  guard("flat", (self, args) => {
    if (self !== null && self !== undefined) {
      scanFlat(self, args[0] === undefined ? 1 : Number(args[0]));
    }
  });
  const flatMap = ArrayProto.flatMap;
  defineProperty(ArrayProto, "flatMap", method({
    flatMap(callback, thisArg) {
      guardLength(this, "flatMap");
      if (typeof callback !== "function") {
        return apply(flatMap, this, [callback, thisArg]);
      }
      return apply(flatMap, this, [
        (...args) => {
          const result = apply(callback, thisArg, args);
          if (isArray(result)) {
            guardLength(result, "flatMap");
          }
          return result;
        },
      ]);
    },
  }.flatMap));
  defineProperty(ArrayProto.flatMap, "length", { value: 1 });

  // ---- 2. loams:server ----

  // Errors the host raised in this call, so the host can return its own
  // error when one escapes the handler.
  const hostErrors = new WeakMap();

  function bytesToLatin1(buffer) {
    const bytes = new Uint8Array(buffer);
    const parts = [];
    for (let i = 0; i < bytes.length; i += 8192) {
      parts.push(apply(fromCharCode, null, bytes.subarray(i, i + 8192)));
    }
    return apply(join, parts, [""]);
  }

  // The helpers the host converts values with: an ArrayBuffer's bytes, and
  // the size of a string or ArrayBuffer, which the host charges against
  // the conversion's budget before converting it.
  const byteLength = getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength").get;
  const values = freeze({
    bytes: bytesToLatin1,
    size: (value) => (typeof value === "string" ? value.length : apply(byteLength, value, [])),
  });

  function call(op, args) {
    const reply = host(op, args, values);
    if ("ok" in reply) {
      return reply.ok;
    }
    if (reply.abort) {
      throw new Error("the call was aborted");
    }
    const error = new Error(reply.error);
    hostErrors.set(error, reply.index);
    throw error;
  }

  function limitOf(n) {
    if (typeof n === "bigint" && n >= 0n) {
      return n;
    }
    if (typeof n === "number" && Number.isInteger(n) && n >= 0) {
      return BigInt(n);
    }
    throw new TypeError(`take(n) needs a non-negative integer, not ${inspect(n, 1, [])}`);
  }

  class IndexRange {
    #fields = [];
    #eq = [];
    #rangeField = undefined;
    #lower = undefined;
    #upper = undefined;

    eq(field, value) {
      if (this.#rangeField !== undefined) {
        throw new TypeError("withIndex: eq() comes before gt/gte/lt/lte");
      }
      this.#fields.push(field);
      this.#eq.push(value);
      return this;
    }

    #bound(side, field, value, inclusive) {
      if (this.#rangeField !== undefined && this.#rangeField !== field) {
        throw new TypeError(`withIndex: both bounds must be on one field, not ${field} and ${this.#rangeField}`);
      }
      if ((side === "lower" ? this.#lower : this.#upper) !== undefined) {
        throw new TypeError(`withIndex: a second ${side} bound on ${field}`);
      }
      this.#rangeField = field;
      if (side === "lower") {
        this.#lower = { value, inclusive };
      } else {
        this.#upper = { value, inclusive };
      }
      return this;
    }

    gt(field, value) {
      return this.#bound("lower", field, value, false);
    }

    gte(field, value) {
      return this.#bound("lower", field, value, true);
    }

    lt(field, value) {
      return this.#bound("upper", field, value, false);
    }

    lte(field, value) {
      return this.#bound("upper", field, value, true);
    }

    static spec(range) {
      const fields = [...range.#fields];
      if (range.#rangeField !== undefined) {
        fields.push(range.#rangeField);
      }
      const spec = { eq: range.#eq, fields };
      if (range.#lower !== undefined) {
        spec.lower = range.#lower;
      }
      if (range.#upper !== undefined) {
        spec.upper = range.#upper;
      }
      return spec;
    }
  }

  class Query {
    #table;
    #index = "by_creation_time";
    #range = { eq: [], fields: [] };
    #order = "asc";
    #indexed = false;

    constructor(table) {
      if (typeof table !== "string") {
        throw new TypeError("db.query(table) needs a table name");
      }
      this.#table = table;
    }

    withIndex(name, build) {
      if (this.#indexed) {
        throw new TypeError("withIndex() is called once per query");
      }
      if (typeof name !== "string") {
        throw new TypeError("withIndex(name, range?) needs an index name");
      }
      this.#indexed = true;
      this.#index = name;
      if (build !== undefined) {
        const range = new IndexRange();
        build(range);
        this.#range = IndexRange.spec(range);
      }
      return this;
    }

    order(order) {
      if (order !== "asc" && order !== "desc") {
        throw new TypeError(`order() is "asc" or "desc", not ${inspect(order, 1, [])}`);
      }
      this.#order = order;
      return this;
    }

    #run(limit) {
      const args = {
        table: this.#table,
        index: this.#index,
        order: this.#order,
        ...this.#range,
      };
      if (limit !== undefined) {
        args.limit = limit;
      }
      return call("query", args);
    }

    async take(n) {
      return this.#run(limitOf(n));
    }

    async collect() {
      return this.#run(undefined);
    }

    async first() {
      const found = this.#run(1n);
      return found.length === 0 ? null : found[0];
    }
  }

  function makeDb(writable) {
    const db = {
      get: async (id) => call("get", { id }),
      query: (table) => new Query(table),
    };
    if (writable) {
      db.insert = async (table, doc) => call("insert", { table, fields: doc });
      db.patch = async (id, fields) => {
        call("patch", { id, fields });
      };
      db.replace = async (id, doc) => {
        call("replace", { id, fields: doc });
      };
      db.delete = async (id) => {
        call("delete", { id });
      };
    }
    return freeze(db);
  }

  const definitions = new WeakSet();

  function define(name, kind, visibility) {
    return {
      [name](definition) {
        let handler;
        let hasArgs = false;
        if (typeof definition === "function") {
          handler = definition;
        } else if (
          definition !== null &&
          typeof definition === "object" &&
          typeof definition.handler === "function"
        ) {
          handler = definition.handler;
          hasArgs = definition.args !== undefined;
        } else {
          throw new TypeError(`${name}() takes a handler function or { args?, handler }`);
        }
        const fn = freeze({ kind, visibility, handler, hasArgs });
        definitions.add(fn);
        return fn;
      },
    }[name];
  }

  const server = freeze({
    query: define("query", "query", "public"),
    mutation: define("mutation", "mutation", "public"),
    internalQuery: define("internalQuery", "query", "internal"),
    internalMutation: define("internalMutation", "mutation", "internal"),
  });

  const table = new Map();

  // The bundle's functions: each own enumerable property of an exported
  // object that a `loams:server` builder made is the function
  // `<export>:<property>`.
  function collect(namespace) {
    const metas = [];
    for (const name of keys(namespace)) {
      const value = namespace[name];
      if (definitions.has(value)) {
        throw new TypeError(
          `export "${name}" is a function; export functions inside a module object: ` +
            `export const <module> = { ${name}: ... }`,
        );
      }
      if (value === null || typeof value !== "object") {
        continue;
      }
      for (const key of keys(value)) {
        const fn = value[key];
        if (!definitions.has(fn)) {
          continue;
        }
        if (name.startsWith("_")) {
          throw new TypeError(`module "${name}": names starting with "_" are reserved`);
        }
        if (name.includes(":") || key.includes(":") || key === "") {
          throw new TypeError(`function "${name}:${key}": a name has no ":" and is not empty`);
        }
        const path = `${name}:${key}`;
        if (fn.hasArgs) {
          throw new TypeError(
            `${path}: argument validators are not supported until LV1 Task 4; leave out args`,
          );
        }
        table.set(path, fn);
        metas.push([path, fn.kind, fn.visibility]);
      }
    }
    return metas;
  }

  // Runs the function `path` with `args`; the result is a promise.
  function invoke(path, args) {
    const fn = table.get(path);
    const ctx = freeze({ db: makeDb(fn.kind === "mutation") });
    return (async () => apply(fn.handler, undefined, [ctx, args]))();
  }

  // ---- 3. The allowlist and the freeze ----

  const allowed = new Set(config.globals);
  for (const name of ownKeys(globalThis)) {
    if (!allowed.has(name)) {
      delete globalThis[name];
    }
  }

  const roots = [];
  for (const name of ownKeys(globalThis)) {
    const desc = getOwnPropertyDescriptor(globalThis, name);
    if ("value" in desc) {
      roots.push(desc.value);
      if (desc.configurable || desc.writable) {
        defineProperty(globalThis, name, { writable: false, configurable: false });
      }
    }
  }
  const hidden = [
    () => function* () {},
    () => async function () {},
    () => async function* () {},
    () => [][Symbol.iterator](),
    () => new Map()[Symbol.iterator](),
    () => new Set()[Symbol.iterator](),
    () => ""[Symbol.iterator](),
    () => /a/[Symbol.matchAll](""),
    () => [].values().map((x) => x),
    () => Iterator.from({ next() {} }),
    () => Object.getPrototypeOf(Int8Array),
  ];
  for (const make of hidden) {
    try {
      roots.push(make());
    } catch (e) {
      // An intrinsic this engine does not have.
    }
  }

  // Override taming: an inherited data property that code commonly assigns
  // becomes an accessor whose setter defines an own property on the target.
  function tame(proto, names) {
    for (const name of names) {
      const desc = getOwnPropertyDescriptor(proto, name);
      if (desc === undefined || !("value" in desc) || !desc.configurable) {
        continue;
      }
      const value = desc.value;
      defineProperty(proto, name, {
        get() {
          return value;
        },
        set(next) {
          if (this === proto) {
            throw new TypeError(`Cannot assign to read only property '${String(name)}'`);
          }
          defineProperty(this, name, {
            value: next,
            writable: true,
            enumerable: true,
            configurable: true,
          });
        },
        enumerable: desc.enumerable,
        configurable: false,
      });
    }
  }
  tame(Object.prototype, ["constructor", "toString", "toLocaleString", "valueOf"]);
  tame(Function.prototype, ["constructor", "toString"]);
  tame(Array.prototype, ["constructor", "toString"]);
  tame(Promise.prototype, ["constructor"]);
  for (const E of [
    Error,
    EvalError,
    RangeError,
    ReferenceError,
    SyntaxError,
    TypeError,
    URIError,
    globalThis.InternalError,
    globalThis.AggregateError,
    globalThis.SuppressedError,
  ]) {
    if (E !== undefined) {
      tame(E.prototype, ["constructor", "name", "message", "toString"]);
    }
  }

  const seen = new Set();
  const stack = roots;
  while (stack.length > 0) {
    const value = stack.pop();
    if (
      (typeof value !== "object" && typeof value !== "function") ||
      value === null ||
      seen.has(value) ||
      value === crypto ||
      value === globalThis
    ) {
      continue;
    }
    seen.add(value);
    stack.push(getPrototypeOf(value));
    for (const key of ownKeys(value)) {
      const desc = getOwnPropertyDescriptor(value, key);
      if ("value" in desc) {
        stack.push(desc.value);
      } else {
        stack.push(desc.get, desc.set);
      }
    }
  }
  for (const value of seen) {
    freeze(value);
  }
  freeze(crypto);

  return freeze({
    server,
    collect,
    invoke,
    hostError: (error) => {
      const index = hostErrors.get(error);
      return index === undefined ? -1 : index;
    },
    values,
  });
});
