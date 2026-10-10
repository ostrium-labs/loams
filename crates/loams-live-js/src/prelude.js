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
//      `internalMutation`, and the validators `v`) with `ctx.db`;
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

  const { now, random, log, host, isProxy } = natives;
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
  const isFinite = Number.isFinite;
  const asIntN = BigInt.asIntN;
  const stringify = JSON.stringify;
  const fromCharCode = String.fromCharCode;
  const trunc = Math.trunc;
  const join = Array.prototype.join;
  const method = (value) => ({ value, writable: true, enumerable: false, configurable: true });

  // ---- 1. Determinism ----

  class DeterminismError extends Error {}
  defineProperty(DeterminismError.prototype, "name", method("DeterminismError"));
  const NO_CRYPTO =
    "crypto randomness is not available in queries and mutations; use an action";

  // Local time is UTC (fix round 1, I1): QuickJS's local-time methods
  // follow the host's time zone, which differs between nodes. Every
  // local-time getter and setter is its UTC twin, `getTimezoneOffset` is 0,
  // the string forms print UTC as "GMT+0000", and a date-time without a
  // zone (in `new Date(y, m, …)`, `Date.parse` and `new Date(string)`) is
  // read as UTC. The parser is QuickJS's own (quickjs.c,
  // js_date_parse_isostring and js_date_parse_otherstring), ported so that
  // it never consults the host's zone.
  const DateProto = OriginalDate.prototype;
  const getTime = DateProto.getTime;
  const UTC = OriginalDate.UTC;
  const utc = {};
  for (const part of ["FullYear", "Month", "Date", "Day", "Hours", "Minutes", "Seconds", "Milliseconds"]) {
    utc["get" + part] = DateProto["getUTC" + part];
    if (part !== "Day") {
      utc["set" + part] = DateProto["setUTC" + part];
    }
  }
  const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const MONTH_CODES = "JANFEBMARAPRMAYJUNJULAUGSEPOCTNOVDEC";
  const ZONES = [
    ["GMT", 0], ["UTC", 0], ["UT", 0], ["Z", 0],
    ["EDT", -4 * 60], ["EST", -5 * 60], ["CDT", -5 * 60], ["CST", -6 * 60],
    ["MDT", -6 * 60], ["MST", -7 * 60], ["PDT", -7 * 60], ["PST", -8 * 60],
    ["WET", 0], ["WEST", 60], ["CET", 60], ["CEST", 2 * 60], ["EET", 2 * 60], ["EEST", 3 * 60],
  ];
  // 400 Gregorian years, in ms: Date.UTC maps years 0-99 to 1900-1999, so
  // those are computed 400 years later and moved back.
  const FOUR_CENTURIES_MS = 146097 * 86400000;

  function pad(n, width) {
    const s = `${n}`;
    return s.length >= width ? s : "0".repeat(width - s.length) + s;
  }

  function year(y) {
    return y < 0 ? "-" + pad(-y, 4) : pad(y, 4);
  }

  // QuickJS's get_date_string with UTC fields: fmt 1 is toString's form,
  // fmt 3 toLocaleString's; part 1 the date, 2 the time, 3 both.
  function dateString(date, fmt, part) {
    const t = apply(getTime, date, []);
    if (t !== t) {
      return "Invalid Date";
    }
    const get = (name) => apply(utc[name], date, []);
    const y = get("getFullYear");
    const mon = get("getMonth");
    const d = get("getDate");
    const h = get("getHours");
    const m = get("getMinutes");
    const s = get("getSeconds");
    let out = "";
    if (part & 1) {
      out += fmt === 1
        ? `${DAYS[get("getDay")]} ${MONTHS[mon]} ${pad(d, 2)} ${year(y)}`
        : `${pad(mon + 1, 2)}/${pad(d, 2)}/${year(y)}`;
      if (part === 3) {
        out += fmt === 1 ? " " : ", ";
      }
    }
    if (part & 2) {
      out += fmt === 1
        ? `${pad(h, 2)}:${pad(m, 2)}:${pad(s, 2)} GMT+0000`
        : `${pad(((h + 11) % 12) + 1, 2)}:${pad(m, 2)}:${pad(s, 2)} ${h < 12 ? "AM" : "PM"}`;
    }
    return out;
  }

  // ---- the date parser, over a NUL-terminated array of char codes ----

  function digits(sp, cur, min, max) {
    let v = 0;
    let p = cur.p;
    const start = p;
    let c;
    while ((c = sp[p]) >= 48 && c <= 57) {
      if (v >= 100000000) {
        return -1;
      }
      v = v * 10 + c - 48;
      p++;
      if (p - start === max) {
        break;
      }
    }
    if (p - start < min) {
      return -1;
    }
    cur.p = p;
    return v;
  }

  function skipChar(sp, cur, c) {
    if (sp[cur.p] === c) {
      cur.p++;
      return true;
    }
    return false;
  }

  function milliseconds(sp, cur, f) {
    let p = cur.p;
    const c0 = sp[p];
    if (c0 === 46 || c0 === 44) {
      p++;
      const start = p;
      let mul = 100;
      let ms = 0;
      let c;
      while ((c = sp[p]) >= 48 && c <= 57) {
        ms += (c - 48) * mul;
        mul = trunc(mul / 10);
        p++;
        if (p - start === 9) {
          break;
        }
      }
      if (p > start) {
        f[6] = ms;
        cur.p = p;
      }
    }
  }

  function tzOffset(sp, cur, f, strict) {
    let p = cur.p;
    const sgn = sp[p++];
    let tz = 0;
    if (sgn === 43 || sgn === 45) {
      const at = { p };
      let hh = digits(sp, at, 1, 0);
      if (hh < 0) {
        return false;
      }
      let n = at.p - p;
      p = at.p;
      if (strict && n !== 2 && n !== 4) {
        return false;
      }
      while (n > 4) {
        n -= 2;
        hh = trunc(hh / 100);
      }
      let mm = 0;
      if (n > 2) {
        mm = hh % 100;
        hh = trunc(hh / 100);
      } else if (skipChar(sp, at, 58)) {
        mm = digits(sp, at, 2, 2);
        if (mm < 0) {
          return false;
        }
        p = at.p;
      }
      if (hh > 23 || mm > 59) {
        return false;
      }
      tz = hh * 60 + mm;
      if (sgn !== 43) {
        tz = -tz;
      }
    } else if (sgn !== 90) {
      return false;
    }
    cur.p = p;
    f[8] = tz;
    return true;
  }

  function upper(c) {
    return c >= 97 && c <= 122 ? c - 32 : c;
  }

  function match(sp, cur, s) {
    let p = cur.p;
    for (let i = 0; i < s.length; i++, p++) {
      if (upper(sp[p]) !== s.charCodeAt(i)) {
        return false;
      }
    }
    cur.p = p;
    return true;
  }

  function month(sp, cur, f) {
    for (let n = 0; n < 12; n++) {
      if (match(sp, { p: cur.p }, MONTH_CODES.slice(n * 3, n * 3 + 3))) {
        f[1] = n + 1;
        cur.p += 3;
        return true;
      }
    }
    return false;
  }

  function skipUntil(sp, cur, stops) {
    let c;
    while ((c = sp[cur.p]) !== 0 && !stops.includes(fromCharCode(c))) {
      cur.p++;
    }
  }

  function parseIso(sp, f) {
    for (let i = 0; i < 9; i++) {
      f[i] = i === 2 ? 1 : 0;
    }
    const cur = { p: 0 };
    const sgn = sp[0];
    if (sgn === 45 || sgn === 43) {
      cur.p++;
      const y = digits(sp, cur, 6, 6);
      if (y < 0 || (sgn === 45 && y === 0)) {
        return false;
      }
      f[0] = sgn === 45 ? -y : y;
    } else {
      const y = digits(sp, cur, 4, 4);
      if (y < 0) {
        return false;
      }
      f[0] = y;
    }
    if (skipChar(sp, cur, 45)) {
      const m = digits(sp, cur, 2, 2);
      if (m < 1) {
        return false;
      }
      f[1] = m - 1;
      if (skipChar(sp, cur, 45)) {
        const d = digits(sp, cur, 2, 2);
        if (d < 1) {
          return false;
        }
        f[2] = d;
      }
    }
    if (skipChar(sp, cur, 84)) {
      const h = digits(sp, cur, 2, 2);
      if (h >= 0) {
        f[3] = h;
      }
      if (h < 0 || !skipChar(sp, cur, 58)) {
        f[3] = 100; // rejected by the range check
        return true;
      }
      const mi = digits(sp, cur, 2, 2);
      if (mi < 0) {
        f[3] = 100;
        return true;
      }
      f[4] = mi;
      if (skipChar(sp, cur, 58)) {
        const sec = digits(sp, cur, 2, 2);
        if (sec < 0) {
          return false;
        }
        f[5] = sec;
        milliseconds(sp, cur, f);
      }
    }
    if (sp[cur.p] !== 0 && !tzOffset(sp, cur, f, true)) {
      return false;
    }
    return sp[cur.p] === 0;
  }

  function century(y) {
    return y + (y < 100) * 1900 + (y < 50) * 100;
  }

  function parseOther(sp, f) {
    f[0] = 2001;
    f[1] = 1;
    f[2] = 1;
    for (let i = 3; i < 9; i++) {
      f[i] = 0;
    }
    const cur = { p: 0 };
    const num = [];
    let hasYear = false;
    let hasMon = false;
    let hasTime = false;
    for (;;) {
      while (sp[cur.p] === 32) {
        cur.p++;
      }
      if (sp[cur.p] === 0) {
        break;
      }
      const start = cur.p;
      const c = sp[start];
      let v;
      if (c === 43 || c === 45) {
        if (!(hasTime && tzOffset(sp, cur, f, false))) {
          cur.p++;
          v = digits(sp, cur, 1, 0);
          if (v >= 0) {
            if (c === 45) {
              if (v === 0) {
                return false;
              }
              v = -v;
            }
            f[0] = v;
            hasYear = true;
          }
        }
      } else if ((v = digits(sp, cur, 1, 0)) >= 0) {
        if (skipChar(sp, cur, 58)) {
          f[3] = v;
          const mi = digits(sp, cur, 1, 2);
          if (mi < 0) {
            return false;
          }
          f[4] = mi;
          if (skipChar(sp, cur, 58)) {
            const sec = digits(sp, cur, 1, 2);
            if (sec < 0) {
              return false;
            }
            f[5] = sec;
            milliseconds(sp, cur, f);
          } else if (sp[cur.p] !== 0 && sp[cur.p] !== 32) {
            return false;
          }
          hasTime = true;
        } else if (cur.p - start > 2) {
          f[0] = v;
          hasYear = true;
        } else if (v < 1 || v > 31) {
          f[0] = century(v);
          hasYear = true;
        } else {
          if (num.length === 3) {
            return false;
          }
          num.push(v);
        }
      } else if (month(sp, cur, f)) {
        hasMon = true;
        skipUntil(sp, cur, "0123456789 -/(");
      } else if (hasTime && match(sp, cur, "PM")) {
        if (f[3] !== 12) {
          f[3] += 12;
        }
        continue;
      } else if (hasTime && match(sp, cur, "AM")) {
        if (f[3] > 12) {
          return false;
        }
        if (f[3] === 12) {
          f[3] -= 12;
        }
        continue;
      } else if (ZONES.some(([name, offset]) => match(sp, cur, name) && ((f[8] = offset), true))) {
        continue;
      } else if (c === 40) {
        let level = 0;
        let ch;
        while ((ch = sp[cur.p]) !== 0) {
          cur.p++;
          level += ch === 40;
          level -= ch === 41;
          if (!level) {
            break;
          }
        }
        if (level > 0) {
          return false;
        }
      } else if (c === 41) {
        return false;
      } else {
        if (hasYear + hasMon + hasTime + num.length) {
          return false;
        }
        skipUntil(sp, cur, " -/(");
      }
      let s;
      while ((s = sp[cur.p]) === 45 || s === 47 || s === 46 || s === 44) {
        cur.p++;
      }
    }
    if (num.length + hasYear + hasMon > 3) {
      return false;
    }
    switch (num.length) {
      case 0:
        if (!hasYear) {
          return false;
        }
        break;
      case 1:
        if (hasMon) {
          f[2] = num[0];
        } else {
          f[1] = num[0];
        }
        break;
      case 2:
        if (hasYear) {
          f[1] = num[0];
          f[2] = num[1];
        } else if (hasMon) {
          f[0] = century(num[1]);
          f[2] = num[0];
        } else {
          f[1] = num[0];
          f[2] = num[1];
        }
        break;
      default:
        f[0] = century(num[2]);
        f[1] = num[0];
        f[2] = num[1];
    }
    if (f[1] < 1 || f[2] < 1) {
      return false;
    }
    f[1] -= 1;
    return true;
  }

  // Date.parse with a zone-less date-time read as UTC.
  function parse(value) {
    const s = `${value}`;
    const sp = [];
    for (let i = 0; i < s.length && i < 127; i++) {
      const c = s.charCodeAt(i);
      sp.push(c > 255 ? (c === 0x2212 ? 45 : 120) : c);
    }
    sp.push(0);
    const f = [0, 0, 1, 0, 0, 0, 0, 0, 0];
    if (!parseIso(sp, f) && !parseOther(sp, f)) {
      return NaN;
    }
    const max = [0, 11, 31, 24, 59, 59];
    for (let i = 1; i < 6; i++) {
      if (f[i] > max[i]) {
        return NaN;
      }
    }
    if (f[3] === 24 && (f[4] | f[5] | f[6])) {
      return NaN;
    }
    const time = f[0] >= 0 && f[0] <= 99
      ? UTC(f[0] + 400, f[1], f[2], f[3], f[4], f[5], f[6]) - FOUR_CENTURIES_MS
      : UTC(f[0], f[1], f[2], f[3], f[4], f[5], f[6]);
    return time - f[8] * 60000;
  }

  function isDate(value) {
    try {
      apply(getTime, value, []);
      return true;
    } catch (e) {
      return false;
    }
  }

  // ToPrimitive(value, default), once.
  function toPrimitive(value) {
    if (value === null || (typeof value !== "object" && typeof value !== "function")) {
      return value;
    }
    const isPrimitive = (r) => r === null || (typeof r !== "object" && typeof r !== "function");
    const exotic = value[Symbol.toPrimitive];
    if (exotic !== undefined && exotic !== null) {
      if (typeof exotic !== "function") {
        throw new TypeError("Symbol.toPrimitive is not a function");
      }
      const r = apply(exotic, value, ["default"]);
      if (!isPrimitive(r)) {
        throw new TypeError("cannot convert an object to a primitive value");
      }
      return r;
    }
    for (const name of ["valueOf", "toString"]) {
      const f = value[name];
      if (typeof f === "function") {
        const r = apply(f, value, []);
        if (isPrimitive(r)) {
          return r;
        }
      }
    }
    throw new TypeError("cannot convert an object to a primitive value");
  }

  function Date(...args) {
    if (new.target === undefined) {
      return dateString(new OriginalDate(now()), 1, 3);
    }
    if (args.length === 0) {
      return construct(OriginalDate, [now()], new.target);
    }
    if (args.length === 1) {
      const value = args[0];
      if (isDate(value)) {
        return construct(OriginalDate, [value], new.target);
      }
      const p = toPrimitive(value);
      return construct(OriginalDate, [typeof p === "string" ? parse(p) : p], new.target);
    }
    return construct(OriginalDate, [apply(UTC, undefined, args)], new.target);
  }
  defineProperty(Date, "length", { value: 7 });
  defineProperty(Date, "prototype", { value: OriginalDate.prototype, writable: false });
  defineProperty(OriginalDate.prototype, "constructor", method(Date));
  defineProperty(Date, "now", method(function now_() {
    return now();
  }));
  defineProperty(Date.now, "name", { value: "now" });
  defineProperty(Date, "parse", method({ parse(string) { return parse(string); } }.parse));
  defineProperty(Date, "UTC", method(UTC));
  defineProperty(globalThis, "Date", method(Date));

  function dateMethod(name, length, body) {
    const shim = { [name]: body }[name];
    defineProperty(shim, "length", { value: length });
    defineProperty(DateProto, name, method(shim));
  }
  for (const part of ["FullYear", "Month", "Date", "Day", "Hours", "Minutes", "Seconds", "Milliseconds"]) {
    const get = utc["get" + part];
    dateMethod("get" + part, 0, function () {
      return apply(get, this, []);
    });
    if (part !== "Day") {
      const set = utc["set" + part];
      dateMethod("set" + part, set.length, function (...args) {
        return apply(set, this, args);
      });
    }
  }
  dateMethod("getYear", 0, function () {
    return apply(utc.getFullYear, this, []) - 1900;
  });
  dateMethod("setYear", 1, function (y) {
    apply(getTime, this, []);
    let value = trunc(Number(y));
    if (value >= 0 && value <= 99) {
      value += 1900;
    }
    return apply(utc.setFullYear, this, [value]);
  });
  dateMethod("getTimezoneOffset", 0, function () {
    const t = apply(getTime, this, []);
    return t !== t ? NaN : 0;
  });
  for (const [name, fmt, part] of [
    ["toString", 1, 3],
    ["toDateString", 1, 1],
    ["toTimeString", 1, 2],
    ["toLocaleString", 3, 3],
    ["toLocaleDateString", 3, 1],
    ["toLocaleTimeString", 3, 2],
  ]) {
    dateMethod(name, 0, function () {
      return dateString(this, fmt, part);
    });
  }

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
  //
  // The guard reads the length the way the built-in will, once, right
  // before calling it, with no user code in between: it refuses a Proxy
  // anywhere on the receiver's prototype chain, a `length` getter (except
  // the typed arrays' own), and a `length` that is an object (its
  // `valueOf` could answer differently the second time). `concat` and
  // `flat` read several lengths, and user code (an element's getter) runs
  // between those reads, so they are written in JavaScript here, where the
  // interrupt handler is polled; they return plain arrays (no
  // `Symbol.species`).
  //
  // Not every C loop is covered: other built-ins (`JSON.stringify` of a
  // huge sparse array, `Array.from` of a huge array-like, string methods)
  // are bounded by the memory limit, not the CPU limit. In-process
  // isolation runs trusted code only (LV1 row T3-10).
  const MAX_LENGTH = config.maxLength;
  const ArrayProto = Array.prototype;
  const typedArrayLength = getOwnPropertyDescriptor(
    getPrototypeOf(Int8Array).prototype,
    "length",
  ).get;
  const SPREADABLE = Symbol.isConcatSpreadable;

  function unsupported(name, what) {
    throw new TypeError(
      `Array.prototype.${name}: ${what} is not supported (the length could change ` +
        "between the check and the built-in's own read)",
    );
  }

  function tooLong(name, length) {
    throw new RangeError(
      `Array.prototype.${name}: an array of length ${length} is longer than functions ` +
        `may process (${MAX_LENGTH}); an array this long is sparse`,
    );
  }

  // The own or inherited property `key` of `target`, refusing a Proxy on
  // the way (its traps are user code).
  function lookup(target, key, name) {
    for (let o = target; o !== null; o = getPrototypeOf(o)) {
      if (isProxy(o)) {
        unsupported(name, "a Proxy");
      }
      const desc = getOwnPropertyDescriptor(o, key);
      if (desc !== undefined) {
        return desc;
      }
    }
    return undefined;
  }

  // The length a built-in will read from `target`, read without running
  // user code; refuses one past MAX_LENGTH.
  function lengthOf(target, name) {
    let length;
    if (target === null || target === undefined) {
      return 0; // The built-in throws its own TypeError.
    } else if (typeof target === "string") {
      length = target.length;
    } else if (typeof target !== "object" && typeof target !== "function") {
      return 0; // Numbers, booleans, symbols and bigints have no length.
    } else {
      const desc = lookup(target, "length", name);
      if (desc === undefined) {
        return 0;
      } else if (!("value" in desc)) {
        if (desc.get !== typedArrayLength) {
          unsupported(name, "a length getter");
        }
        length = apply(typedArrayLength, target, []);
      } else {
        const value = desc.value;
        if (value !== null && (typeof value === "object" || typeof value === "function")) {
          unsupported(name, "a length that is an object");
        }
        length = typeof value === "bigint" || typeof value === "symbol" ? 0 : Number(value);
      }
    }
    if (length > MAX_LENGTH) {
      tooLong(name, length);
    }
    return length;
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
    "fill",
    "with",
    "toReversed",
    "toSorted",
    "toSpliced",
  ]) {
    guard(name, (self) => lengthOf(self, name));
  }

  function isSpreadable(value, name) {
    if (value === null || (typeof value !== "object" && typeof value !== "function")) {
      return false;
    }
    const desc = lookup(value, SPREADABLE, name);
    if (desc !== undefined) {
      if (!("value" in desc)) {
        unsupported(name, "a Symbol.isConcatSpreadable getter");
      }
      if (desc.value !== undefined) {
        return !!desc.value;
      }
    }
    return isArray(value);
  }

  defineProperty(ArrayProto, "concat", method({
    concat(...items) {
      if (this === null || this === undefined) {
        throw new TypeError("Array.prototype.concat called on null or undefined");
      }
      const out = [];
      let n = 0;
      const sources = [Object(this), ...items];
      for (let s = 0; s < sources.length; s++) {
        const source = sources[s];
        if (isSpreadable(source, "concat")) {
          const length = lengthOf(source, "concat");
          for (let k = 0; k < length; k++, n++) {
            if (k in source) {
              out[n] = source[k];
            }
          }
        } else {
          out[n] = source;
          n++;
        }
      }
      out.length = n;
      return out;
    },
  }.concat));
  defineProperty(ArrayProto.concat, "length", { value: 1 });

  function flattenInto(out, source, length, start, depth) {
    let n = start;
    for (let i = 0; i < length; i++) {
      if (!(i in source)) {
        continue;
      }
      const item = source[i];
      if (depth > 0 && isArray(item)) {
        n = flattenInto(out, item, lengthOf(item, "flat"), n, depth - 1);
      } else {
        out[n] = item;
        n++;
      }
    }
    return n;
  }

  defineProperty(ArrayProto, "flat", method({
    flat(depthArg) {
      if (this === null || this === undefined) {
        throw new TypeError("Array.prototype.flat called on null or undefined");
      }
      const source = Object(this);
      const length = lengthOf(source, "flat");
      let depth = 1;
      if (depthArg !== undefined) {
        const d = Number(depthArg);
        depth = d !== d || d < 0 ? 0 : trunc(d);
      }
      const out = [];
      flattenInto(out, source, length, 0, depth);
      return out;
    },
  }.flat));
  defineProperty(ArrayProto.flat, "length", { value: 0 });

  const flatMap = ArrayProto.flatMap;
  defineProperty(ArrayProto, "flatMap", method({
    flatMap(callback, thisArg) {
      lengthOf(this, "flatMap");
      if (typeof callback !== "function") {
        return apply(flatMap, this, [callback, thisArg]);
      }
      return apply(flatMap, this, [
        (...args) => {
          const result = apply(callback, thisArg, args);
          if (isArray(result)) {
            lengthOf(result, "flatMap");
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

  // A count as a bigint: an integer from `min`, as a number or a bigint.
  function countOf(n, what, min) {
    if (typeof n === "bigint" && n >= BigInt(min)) {
      return n;
    }
    if (typeof n === "number" && Number.isInteger(n) && n >= min) {
      return BigInt(n);
    }
    const kind = min === 0 ? "a non-negative" : "a positive";
    throw new TypeError(`${what} needs ${kind} integer, not ${inspect(n, 1, [])}`);
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

    #spec() {
      return {
        table: this.#table,
        index: this.#index,
        order: this.#order,
        ...this.#range,
      };
    }

    #run(limit) {
      const args = this.#spec();
      if (limit !== undefined) {
        args.limit = limit;
      }
      return call("query", args);
    }

    async take(n) {
      return this.#run(countOf(n, "take(n)", 0));
    }

    // One page: `{ page, continueCursor, isDone }` (LV1 Task 4). The cursor
    // is null for the first page, then the previous page's
    // `continueCursor`; the host refuses one it did not issue.
    async paginate(options) {
      if (options === null || typeof options !== "object") {
        throw new TypeError(
          `paginate({ cursor, numItems }) takes an object, not ${inspect(options, 1, [])}`,
        );
      }
      const { cursor, numItems } = options;
      if (cursor !== null && cursor !== undefined && typeof cursor !== "string") {
        throw new TypeError(`paginate: cursor is a string or null, not ${inspect(cursor, 1, [])}`);
      }
      return call("paginate", {
        ...this.#spec(),
        cursor: cursor ?? null,
        numItems: countOf(numItems, "paginate: numItems", 1),
      });
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
        // R1 cannot remove a field by patching (row T8-9); Convex removes
        // a field patched to `undefined`, so refuse it rather than keep the
        // field silently.
        if (fields !== null && typeof fields === "object") {
          for (const key of keys(fields)) {
            if (fields[key] === undefined) {
              throw new TypeError(
                `ctx.db.patch: field "${key}" is undefined; a patch cannot remove a field ` +
                  "yet, use ctx.db.replace with the document without it",
              );
            }
          }
        }
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

  // ---- 2. Validators: `v` (LV1 Task 4) ----

  // A validator is a frozen descriptor (`{ kind, … }`) that `v` made; the
  // host reads it when the bundle loads (validators.rs) and checks each
  // call's arguments before the handler runs.
  const validators = new WeakSet();

  function validator(descriptor) {
    const made = freeze(descriptor);
    validators.add(made);
    return made;
  }

  function needValidator(value, where) {
    if (!validators.has(value)) {
      throw new TypeError(
        `${where} needs a validator (v.string(), v.object({ … }), …), not ${inspect(value, 1, [])}`,
      );
    }
    return value;
  }

  function validatorFields(fields, where) {
    if (fields === null || typeof fields !== "object" || isArray(fields)) {
      throw new TypeError(`${where} takes an object of validators, not ${inspect(fields, 1, [])}`);
    }
    const out = {};
    for (const key of keys(fields)) {
      // Defined, not assigned: a field named `__proto__` is a field.
      defineProperty(out, key, {
        value: needValidator(fields[key], `${where}: field ${stringify(key)}`),
        enumerable: true,
      });
    }
    return freeze(out);
  }

  function literalOf(value) {
    switch (typeof value) {
      case "string":
      case "boolean":
        return value;
      case "number":
        if (!isFinite(value)) {
          throw new TypeError(`v.literal() takes a finite number, not ${value}`);
        }
        return value;
      case "bigint":
        if (asIntN(64, value) !== value) {
          throw new TypeError(`v.literal(): the bigint ${value}n is outside int64`);
        }
        return value;
      default:
        if (value === null) {
          return null;
        }
        throw new TypeError(
          `v.literal() takes a string, number, bigint, boolean or null, not ${inspect(value, 1, [])}`,
        );
    }
  }

  const scalar = (kind) => () => validator({ kind });
  const v = freeze({
    null: scalar("null"),
    int64: scalar("int64"),
    float64: scalar("float64"),
    boolean: scalar("boolean"),
    string: scalar("string"),
    bytes: scalar("bytes"),
    any: scalar("any"),
    array: (element) => validator({ kind: "array", element: needValidator(element, "v.array()") }),
    object: (fields) => validator({ kind: "object", fields: validatorFields(fields, "v.object()") }),
    literal: (value) => validator({ kind: "literal", value: literalOf(value) }),
    union: (...members) => {
      if (members.length === 0) {
        throw new TypeError("v.union() needs at least one member");
      }
      return validator({
        kind: "union",
        members: freeze(members.map((m, i) => needValidator(m, `v.union() member ${i}`))),
      });
    },
    optional: (inner) => validator({ kind: "optional", inner: needValidator(inner, "v.optional()") }),
    id: (table) => {
      if (typeof table !== "string" || table === "") {
        throw new TypeError(`v.id(table) needs a table name, not ${inspect(table, 1, [])}`);
      }
      return validator({ kind: "id", table });
    },
  });

  // A function's `args`: an object of validators, or `v.object({ … })`.
  function argsOf(args, name) {
    if (validators.has(args)) {
      if (args.kind !== "object") {
        throw new TypeError(
          `${name}(): args is an object of validators or v.object({ … }), not v.${args.kind}()`,
        );
      }
      return args;
    }
    return validator({ kind: "object", fields: validatorFields(args, `${name}(): args`) });
  }

  const definitions = new WeakSet();

  function define(name, kind, visibility) {
    return {
      [name](definition) {
        let handler;
        let args;
        if (typeof definition === "function") {
          handler = definition;
        } else if (
          definition !== null &&
          typeof definition === "object" &&
          typeof definition.handler === "function"
        ) {
          handler = definition.handler;
          if (definition.args !== undefined) {
            args = argsOf(definition.args, name);
          }
        } else {
          throw new TypeError(`${name}() takes a handler function or { args?, handler }`);
        }
        const fn = freeze({ kind, visibility, handler, args });
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
    v,
  });

  const table = new Map();

  // The bundle's functions: each own enumerable property of an exported
  // object that a `loams:server` builder made is the function
  // `<export>:<property>`. A row is `[path, kind, visibility, args]`, args
  // the validator descriptor or undefined.
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
        table.set(path, fn);
        metas.push([path, fn.kind, fn.visibility, fn.args]);
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
