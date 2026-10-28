// The prelude of the agent profile (SPIKE, MS1 Task 1, D1807). It runs in
// a fresh context before the bundle and returns the internals the worker
// loop calls with incoming frames.
//
// Unlike Live's function prelude this profile is not deterministic: `Date`
// and `performance.now()` are real (`clock_gettime` is allowed). Every
// other effect is a frame to agentd:
//
//   - ACP: `acp.connect(handler)` and `acp.stream()` (Web-stream shim for
//     the ACP TypeScript SDK) send and receive `AcpMessage` frames;
//   - `fetch` (agentd's egress proxy), `model.chat/responses/messages`
//     (agentd's model proxy), `mcp.call/list` (agentd's MCP shim) and
//     `loams.token()` are `AgentCall`s answered by `AgentReply` (and
//     `BodyChunk`s);
//   - timers are `TimerSet`/`TimerClear`, fired by `TimerFired`;
//   - `Math.random` and `crypto` draw from a ChaCha20 stream seeded by the
//     host (`AgentLoad.entropy`) and reseeded by `Entropy` frames.
//
// No `eval`, no `Function` constructor (nor the async and generator ones),
// no `WebAssembly`, no Node built-ins. Globals added: setTimeout,
// clearTimeout, setInterval, clearInterval, queueMicrotask, console,
// crypto, TextEncoder, TextDecoder, URL, URLSearchParams, Headers, Request,
// Response, AbortController, AbortSignal, structuredClone, fetch.

(function setup(natives, config) {
  "use strict";

  const N = natives;
  const g = globalThis;
  const stringify = JSON.stringify;
  const define = (obj, name, value) =>
    Object.defineProperty(obj, name, { value, writable: true, configurable: true, enumerable: false });

  // ---- host calls ----
  const pending = new Map(); // id -> { resolve, reject, onChunk }
  let nextId = 1;
  class HostError extends Error {}
  define(HostError.prototype, "name", "HostError");

  function hostCall(start, onChunk) {
    const id = nextId++;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject, onChunk });
      start(id);
    });
  }
  // `AgentReply`: kind is "error", "json" or "fetch" (payload: the head as JSON).
  function onReply(id, kind, payload) {
    const p = pending.get(id);
    if (!p) return;
    if (kind === "fetch") {
      // The body follows as chunks; the call stays pending for them.
      p.resolve(JSON.parse(payload));
      if (!p.onChunk) pending.delete(id);
      return;
    }
    pending.delete(id);
    if (kind === "error") p.reject(new HostError(payload));
    else p.resolve(payload === "" ? undefined : JSON.parse(payload));
  }
  // `BodyChunk`: `data` is an ArrayBuffer.
  function onChunk(id, data, last) {
    const p = pending.get(id);
    if (!p) return;
    if (last) pending.delete(id);
    if (p.onChunk) p.onChunk(new Uint8Array(data), last);
  }

  // ---- timers ----
  const timers = new Map(); // id -> { fn, args, interval }
  let nextTimer = 1;
  function setTimer(fn, ms, args, interval) {
    if (typeof fn !== "function") throw new TypeError("a timer needs a function");
    const id = nextTimer++;
    const delay = Math.max(0, Math.floor(Number(ms) || 0));
    timers.set(id, { fn, args, interval, delay });
    N.timerSet(id, delay);
    return id;
  }
  function onTimer(id) {
    const t = timers.get(id);
    if (!t) return;
    if (t.interval) N.timerSet(id, t.delay);
    else timers.delete(id);
    t.fn(...t.args);
  }
  function clearTimer(id) {
    if (timers.delete(id)) N.timerClear(id);
  }

  // ---- console ----
  const show = (v) => {
    if (typeof v === "string") return v;
    if (v instanceof Error) return String(v) + (v.stack ? "\n" + v.stack : "");
    try {
      return stringify(v);
    } catch {
      return String(v);
    }
  };
  const console = {};
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    console[level] = (...args) => N.log(level, args.map(show).join(" "));
  }

  // ---- text ----
  class TextEncoder {
    get encoding() {
      return "utf-8";
    }
    encode(s = "") {
      return new Uint8Array(N.utf8(String(s)));
    }
  }
  function decodeUtf8(bytes) {
    let out = "";
    const parts = [];
    for (let i = 0; i < bytes.length; ) {
      const b = bytes[i];
      let cp;
      let n = 1;
      if (b < 0x80) cp = b;
      else if (b >= 0xc2 && b < 0xe0 && i + 1 < bytes.length) {
        cp = ((b & 0x1f) << 6) | (bytes[i + 1] & 0x3f);
        n = 2;
      } else if (b >= 0xe0 && b < 0xf0 && i + 2 < bytes.length) {
        cp = ((b & 0x0f) << 12) | ((bytes[i + 1] & 0x3f) << 6) | (bytes[i + 2] & 0x3f);
        n = 3;
      } else if (b >= 0xf0 && b < 0xf5 && i + 3 < bytes.length) {
        cp = ((b & 0x07) << 18) | ((bytes[i + 1] & 0x3f) << 12) | ((bytes[i + 2] & 0x3f) << 6) | (bytes[i + 3] & 0x3f);
        n = 4;
      } else cp = 0xfffd;
      out += String.fromCodePoint(cp);
      i += n;
      if (out.length > 8192) {
        parts.push(out);
        out = "";
      }
    }
    parts.push(out);
    return parts.join("");
  }
  class TextDecoder {
    constructor(label = "utf-8") {
      if (!/^utf-?8$/i.test(String(label))) throw new RangeError("only utf-8 is supported");
    }
    get encoding() {
      return "utf-8";
    }
    decode(input) {
      if (input === undefined) return "";
      const bytes = input instanceof ArrayBuffer ? new Uint8Array(input) : new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
      const s = decodeUtf8(bytes);
      return s.charCodeAt(0) === 0xfeff ? s.slice(1) : s;
    }
  }
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const toBytes = (body) => {
    if (body === undefined || body === null) return null;
    if (typeof body === "string") return encoder.encode(body);
    if (body instanceof ArrayBuffer) return new Uint8Array(body);
    if (ArrayBuffer.isView(body)) return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
    if (body instanceof URLSearchParams) return encoder.encode(body.toString());
    return encoder.encode(String(body));
  };
  const latin1 = (bytes) => {
    const parts = [];
    for (let i = 0; i < bytes.length; i += 8192) parts.push(String.fromCharCode.apply(null, bytes.subarray(i, i + 8192)));
    return parts.join("");
  };
  const concat = (chunks) => {
    let n = 0;
    for (const c of chunks) n += c.length;
    const out = new Uint8Array(n);
    let at = 0;
    for (const c of chunks) {
      out.set(c, at);
      at += c.length;
    }
    return out;
  };

  // ---- crypto ----
  const crypto = {
    getRandomValues(view) {
      if (!ArrayBuffer.isView(view)) throw new TypeError("getRandomValues needs a typed array");
      if (view.byteLength > 65536) throw new RangeError("at most 65536 bytes");
      new Uint8Array(view.buffer, view.byteOffset, view.byteLength).set(new Uint8Array(N.randomBytes(view.byteLength)));
      return view;
    },
    randomUUID() {
      const b = new Uint8Array(N.randomBytes(16));
      b[6] = (b[6] & 0x0f) | 0x40;
      b[8] = (b[8] & 0x3f) | 0x80;
      const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
      return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
    },
  };

  // ---- URL (WHATWG subset: special schemes, no IDNA) ----
  class URLSearchParams {
    constructor(init = "") {
      this._list = [];
      if (typeof init === "string") {
        for (const part of init.replace(/^\?/, "").split("&")) {
          if (!part) continue;
          const i = part.indexOf("=");
          const dec = (s) => decodeURIComponent(s.replace(/\+/g, " "));
          this._list.push(i < 0 ? [dec(part), ""] : [dec(part.slice(0, i)), dec(part.slice(i + 1))]);
        }
      } else if (init && typeof init[Symbol.iterator] === "function") {
        for (const [k, v] of init) this._list.push([String(k), String(v)]);
      } else if (init && typeof init === "object") {
        for (const k of Object.keys(init)) this._list.push([k, String(init[k])]);
      }
    }
    append(k, v) { this._list.push([String(k), String(v)]); this._update(); }
    delete(k) { this._list = this._list.filter(([x]) => x !== k); this._update(); }
    get(k) { const e = this._list.find(([x]) => x === k); return e ? e[1] : null; }
    getAll(k) { return this._list.filter(([x]) => x === k).map(([, v]) => v); }
    has(k) { return this._list.some(([x]) => x === k); }
    set(k, v) {
      const i = this._list.findIndex(([x]) => x === k);
      if (i < 0) this._list.push([String(k), String(v)]);
      else {
        this._list[i] = [String(k), String(v)];
        this._list = this._list.filter(([x], j) => x !== k || j === i);
      }
      this._update();
    }
    forEach(fn) { for (const [k, v] of this._list) fn(v, k, this); }
    keys() { return this._list.map(([k]) => k)[Symbol.iterator](); }
    values() { return this._list.map(([, v]) => v)[Symbol.iterator](); }
    entries() { return this._list.map((e) => e.slice())[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    toString() {
      const enc = (s) => encodeURIComponent(s).replace(/%20/g, "+");
      return this._list.map(([k, v]) => `${enc(k)}=${enc(v)}`).join("&");
    }
    _update() { if (this._url) this._url._search = this._list.length ? "?" + this.toString() : ""; }
  }
  const DEFAULT_PORTS = { "http:": "80", "https:": "443", "ws:": "80", "wss:": "443" };
  const URL_RE = /^([a-zA-Z][a-zA-Z0-9+.-]*:)(?:\/\/(?:([^:@\/?#]*)(?::([^@\/?#]*))?@)?(\[[^\]]*\]|[^:\/?#]*)(?::(\d*))?)?([^?#]*)(\?[^#]*)?(#.*)?$/;
  class URL {
    constructor(input, base) {
      let s = String(input).trim();
      if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(s)) {
        if (base === undefined) throw new TypeError(`Invalid URL: ${s}`);
        const b = new URL(base);
        if (s.startsWith("//")) s = b.protocol + s;
        else if (s.startsWith("/")) s = b.origin + s;
        else if (s.startsWith("?")) s = b.origin + b.pathname + s;
        else if (s.startsWith("#")) s = b.origin + b.pathname + b.search + s;
        else s = b.origin + b.pathname.replace(/[^/]*$/, "") + s;
      }
      const m = URL_RE.exec(s);
      if (!m) throw new TypeError(`Invalid URL: ${s}`);
      this.protocol = m[1].toLowerCase();
      this.username = m[2] || "";
      this.password = m[3] || "";
      this.hostname = (m[4] || "").toLowerCase();
      const port = m[5] || "";
      this.port = DEFAULT_PORTS[this.protocol] === port ? "" : port;
      if (DEFAULT_PORTS[this.protocol] !== undefined && !this.hostname) throw new TypeError(`Invalid URL: ${s}`);
      let path = m[6] || (this.hostname ? "/" : "");
      const segments = [];
      for (const seg of path.split("/").slice(1)) {
        if (seg === "..") segments.pop();
        else if (seg !== ".") segments.push(seg);
      }
      this.pathname = m[4] !== undefined ? "/" + segments.join("/") : path;
      this._search = m[7] && m[7] !== "?" ? m[7] : "";
      this.hash = m[8] && m[8] !== "#" ? m[8] : "";
    }
    get host() { return this.port ? `${this.hostname}:${this.port}` : this.hostname; }
    get origin() { return `${this.protocol}//${this.host}`; }
    get search() { return this._search; }
    set search(v) { v = String(v); this._search = v && v !== "?" ? (v.startsWith("?") ? v : "?" + v) : ""; this._params = undefined; }
    get searchParams() {
      if (!this._params) {
        this._params = new URLSearchParams(this._search);
        this._params._url = this;
      }
      return this._params;
    }
    get href() {
      const auth = this.username ? `${this.username}${this.password ? ":" + this.password : ""}@` : "";
      return `${this.protocol}//${auth}${this.host}${this.pathname}${this._search}${this.hash}`;
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static canParse(u, b) { try { new URL(u, b); return true; } catch { return false; } }
  }

  // ---- Headers, Request, Response ----
  class Headers {
    constructor(init) {
      this._map = new Map();
      if (init instanceof Headers) init.forEach((v, k) => this.append(k, v));
      else if (Array.isArray(init)) for (const [k, v] of init) this.append(k, v);
      else if (init && typeof init === "object") for (const k of Object.keys(init)) this.append(k, init[k]);
    }
    append(k, v) {
      k = String(k).toLowerCase();
      const old = this._map.get(k);
      this._map.set(k, old === undefined ? String(v) : `${old}, ${v}`);
    }
    set(k, v) { this._map.set(String(k).toLowerCase(), String(v)); }
    get(k) { const v = this._map.get(String(k).toLowerCase()); return v === undefined ? null : v; }
    has(k) { return this._map.has(String(k).toLowerCase()); }
    delete(k) { this._map.delete(String(k).toLowerCase()); }
    forEach(fn) { for (const [k, v] of [...this._map].sort()) fn(v, k, this); }
    entries() { return [...this._map].sort()[Symbol.iterator](); }
    keys() { return [...this._map.keys()].sort()[Symbol.iterator](); }
    values() { return [...this._map].sort().map(([, v]) => v)[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
  }
  // A body: whole bytes, or chunks still arriving from the host.
  class Body {
    _init(bytes, stream) {
      this._bytes = bytes;
      this._stream = stream; // { chunks, done, waiters }
      this.bodyUsed = false;
    }
    async arrayBuffer() {
      if (this.bodyUsed) throw new TypeError("body already used");
      this.bodyUsed = true;
      if (this._stream) {
        const s = this._stream;
        while (!s.done) await new Promise((r) => s.waiters.push(r));
        if (s.error) throw s.error;
        const all = concat(s.chunks);
        return all.buffer.slice(all.byteOffset, all.byteOffset + all.byteLength);
      }
      const b = this._bytes || new Uint8Array(0);
      return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
    }
    async bytes() { return new Uint8Array(await this.arrayBuffer()); }
    async text() { return decoder.decode(await this.arrayBuffer()); }
    async json() { return JSON.parse(await this.text()); }
    get body() {
      // A ReadableStream when the bundle provides one (a polyfill).
      if (typeof g.ReadableStream !== "function") return null;
      const self = this;
      return new g.ReadableStream({
        async start(controller) {
          const b = new Uint8Array(await self.arrayBuffer());
          if (b.length) controller.enqueue(b);
          controller.close();
        },
      });
    }
  }
  class Request extends Body {
    constructor(input, init = {}) {
      super();
      const from = input instanceof Request ? input : null;
      this.url = from ? from.url : new URL(String(input)).href;
      this.method = String(init.method || (from && from.method) || "GET").toUpperCase();
      this.headers = new Headers(init.headers || (from && from.headers));
      this.signal = init.signal || (from && from.signal) || null;
      this._init(init.body !== undefined ? toBytes(init.body) : from ? from._bytes : null, null);
    }
    clone() { return new Request(this); }
  }
  class Response extends Body {
    constructor(body, init = {}) {
      super();
      this.status = init.status === undefined ? 200 : init.status;
      this.statusText = init.statusText || "";
      this.headers = new Headers(init.headers);
      this.url = init.url || "";
      this.type = "basic";
      this.redirected = false;
      this._init(init._stream ? null : toBytes(body), init._stream || null);
    }
    get ok() { return this.status >= 200 && this.status < 300; }
    clone() { throw new TypeError("Response.clone is not supported in the spike"); }
    static json(data, init = {}) {
      const headers = new Headers(init.headers);
      if (!headers.has("content-type")) headers.set("content-type", "application/json");
      return new Response(stringify(data), { ...init, headers });
    }
    static error() { return new Response(null, { status: 0 }); }
  }

  // ---- AbortController ----
  class DOMException extends Error {
    constructor(message, name = "Error") {
      super(message);
      define(this, "name", name);
    }
  }
  class AbortSignal {
    constructor() {
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
      this._listeners = [];
    }
    addEventListener(type, fn) { if (type === "abort") this._listeners.push(fn); }
    removeEventListener(type, fn) { this._listeners = this._listeners.filter((l) => l !== fn); }
    throwIfAborted() { if (this.aborted) throw this.reason; }
    static abort(reason) { const c = new AbortController(); c.abort(reason); return c.signal; }
    static timeout(ms) { const c = new AbortController(); setTimer(() => c.abort(new DOMException("timed out", "TimeoutError")), ms, [], false); return c.signal; }
  }
  class AbortController {
    constructor() { this.signal = new AbortSignal(); }
    abort(reason) {
      const s = this.signal;
      if (s.aborted) return;
      s.aborted = true;
      s.reason = reason === undefined ? new DOMException("This operation was aborted", "AbortError") : reason;
      const event = { type: "abort", target: s };
      for (const fn of [...s._listeners, s.onabort]) {
        if (typeof fn === "function") {
          try { fn(event); } catch (e) { console.error(e); }
        }
      }
    }
  }

  // ---- structuredClone (data only) ----
  function structuredClone(value, seen = new Map()) {
    if (typeof value !== "object" || value === null) {
      if (typeof value === "function" || typeof value === "symbol") throw new DOMException("could not be cloned", "DataCloneError");
      return value;
    }
    if (seen.has(value)) return seen.get(value);
    let out;
    if (Array.isArray(value)) {
      out = [];
      seen.set(value, out);
      for (const v of value) out.push(structuredClone(v, seen));
    } else if (value instanceof Date) out = new Date(value.getTime());
    else if (value instanceof RegExp) out = new RegExp(value.source, value.flags);
    else if (value instanceof ArrayBuffer) out = value.slice(0);
    else if (ArrayBuffer.isView(value)) out = new value.constructor(value);
    else if (value instanceof Map) {
      out = new Map();
      seen.set(value, out);
      for (const [k, v] of value) out.set(structuredClone(k, seen), structuredClone(v, seen));
    } else if (value instanceof Set) {
      out = new Set();
      seen.set(value, out);
      for (const v of value) out.add(structuredClone(v, seen));
    } else {
      out = {};
      seen.set(value, out);
      for (const k of Object.keys(value)) out[k] = structuredClone(value[k], seen);
    }
    seen.set(value, out);
    return out;
  }

  // ---- fetch: through agentd's egress proxy ----
  const MAX_CHUNK = 1 << 20;
  async function fetch(input, init = {}) {
    const request = new Request(input, init);
    const signal = request.signal;
    if (signal) signal.throwIfAborted();
    const headers = [];
    request.headers.forEach((v, k) => headers.push([k, v]));
    const body = request._bytes;
    const stream = { chunks: [], done: false, waiters: [], error: null };
    const wake = () => {
      const w = stream.waiters;
      stream.waiters = [];
      for (const r of w) r();
    };
    let callId = 0;
    const head = hostCall(
      (id) => {
        callId = id;
        N.fetch(id, request.url, request.method, stringify(headers), body !== null && body.length > 0);
        if (body !== null && body.length > 0) {
          for (let at = 0; at < body.length; at += MAX_CHUNK) {
            const piece = body.subarray(at, at + MAX_CHUNK);
            N.bodyChunk(id, latin1(piece), at + MAX_CHUNK >= body.length);
          }
        }
      },
      (chunk, last) => {
        if (chunk.length) stream.chunks.push(chunk);
        if (last) stream.done = true;
        wake();
      },
    );
    let aborted = null;
    if (signal) {
      aborted = () => {
        const p = pending.get(callId);
        pending.delete(callId);
        stream.error = signal.reason;
        stream.done = true;
        wake();
        if (p) p.reject(signal.reason);
      };
      signal.addEventListener("abort", aborted);
    }
    const h = await head;
    return new Response(null, { status: h.status, statusText: h.statusText, headers: h.headers, url: h.url, _stream: stream });
  }

  // ---- loams:agent ----
  // A model call; `stream: true` returns an async iterable of events with
  // `.result` (the final response).
  function modelCall(api) {
    return (request, options = {}) => {
      if (!options.stream) {
        return hostCall((id) => N.model(id, api, stringify(request), false));
      }
      const queue = [];
      let wakeUp = null;
      let done = false;
      let error;
      const poke = () => {
        if (wakeUp) {
          const w = wakeUp;
          wakeUp = null;
          w();
        }
      };
      const result = hostCall(
        (id) => N.model(id, api, stringify(request), true),
        (chunk) => {
          if (chunk.length) queue.push(JSON.parse(decoder.decode(chunk)));
          poke();
        },
      );
      result.then(
        () => { done = true; poke(); },
        (e) => { done = true; error = e; poke(); },
      );
      return {
        result,
        async *[Symbol.asyncIterator]() {
          for (;;) {
            if (queue.length) { yield queue.shift(); continue; }
            if (done) { if (error) throw error; return; }
            await new Promise((r) => { wakeUp = r; });
          }
        },
      };
    };
  }
  const model = Object.freeze({ chat: modelCall("chat"), responses: modelCall("responses"), messages: modelCall("messages") });
  const mcp = Object.freeze({
    call: (server, tool, args = {}) => hostCall((id) => N.mcp(id, "call", String(server), String(tool), stringify(args))),
    list: (server) => hostCall((id) => N.mcp(id, "list", String(server), "", "{}")),
  });
  const loams = Object.freeze({
    token: () => hostCall((id) => N.token(id)),
    config: () => ({ ...config }),
  });

  // ACP: one JSON-RPC message per `AcpMessage` frame.
  let acpHandler = null;
  const acpBacklog = [];
  const acp = Object.freeze({
    // `handler(message)` gets each message from agentd; the result sends.
    connect(handler) {
      if (acpHandler) throw new Error("acp is already connected");
      acpHandler = handler;
      for (const m of acpBacklog.splice(0)) deliverAcp(m);
      return Object.freeze({ send: (message) => N.acp(stringify(message)) });
    },
    // Web streams of newline-delimited JSON bytes, for the ACP TypeScript
    // SDK's `ndJsonStream(output, input)`. Needs ReadableStream and
    // WritableStream (a polyfill the bundle installs).
    stream() {
      if (typeof g.ReadableStream !== "function" || typeof g.WritableStream !== "function") {
        throw new TypeError("acp.stream() needs ReadableStream and WritableStream (bundle a polyfill)");
      }
      let controller;
      const input = new g.ReadableStream({ start(c) { controller = c; } });
      acp.connect((message) => controller.enqueue(encoder.encode(stringify(message) + "\n")));
      let buffered = "";
      const output = new g.WritableStream({
        write(chunk) {
          buffered += typeof chunk === "string" ? chunk : decoder.decode(chunk);
          let nl;
          while ((nl = buffered.indexOf("\n")) >= 0) {
            const line = buffered.slice(0, nl).trim();
            buffered = buffered.slice(nl + 1);
            if (line) N.acp(line);
          }
        },
      });
      return { input, output };
    },
  });
  function deliverAcp(message) {
    Promise.resolve()
      .then(() => acpHandler(message))
      .catch((e) => console.error("acp handler:", e));
  }
  function onAcp(json) {
    let message;
    try {
      message = JSON.parse(json);
    } catch {
      return;
    }
    if (acpHandler) deliverAcp(message);
    else acpBacklog.push(message);
  }

  // ---- the globals ----
  const added = {
    setTimeout: (fn, ms, ...args) => setTimer(fn, ms, args, false),
    setInterval: (fn, ms, ...args) => setTimer(fn, ms, args, true),
    clearTimeout: clearTimer,
    clearInterval: clearTimer,
    queueMicrotask: (fn) => { Promise.resolve().then(fn); },
    console,
    crypto,
    TextEncoder,
    TextDecoder,
    URL,
    URLSearchParams,
    Headers,
    Request,
    Response,
    AbortController,
    AbortSignal,
    DOMException,
    structuredClone: (v) => structuredClone(v),
    fetch,
    performance: g.performance || { now: () => Date.now() },
  };
  for (const k of Object.keys(added)) define(g, k, added[k]);
  Math.random = N.random;

  // No code from strings: `eval` and every Function constructor throw.
  const noCode = function () {
    throw new EvalError("code generation from strings is not allowed in a Loams agent");
  };
  delete g.eval;
  // `Function.prototype` stays reachable as `Function.prototype` (code
  // calls `Function.prototype.apply.call(…)`); only construction throws.
  noCode.prototype = Function.prototype;
  for (const f of [function () {}, async function () {}, function* () {}, async function* () {}]) {
    const proto = Object.getPrototypeOf(f);
    define(proto, "constructor", noCode);
    Object.freeze(proto);
  }
  define(g, "Function", noCode);
  for (const name of ["WebAssembly", "require", "process", "Bun", "Deno"]) delete g[name];

  const agent = Object.freeze({ acp, model, mcp, loams });

  // `ns` is the bundle's module namespace: its `default` (or the
  // namespace) may carry `name` and `version`.
  function bind(ns) {
    const d = (ns && ns.default) || ns || {};
    return { name: String(d.name || ns.name || "agent"), version: String(d.version || ns.version || "0.0.0") };
  }

  return { agent, onAcp, onReply, onChunk, onTimer, bind };
})
