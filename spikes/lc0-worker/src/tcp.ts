// Optional probe (design 79 §8, Q1304): can a Worker reach the TiDB Starter MySQL endpoint over
// `connect()` TCP without Hyperdrive? A minimal MySQL client: greeting, SSLRequest, STARTTLS,
// HandshakeResponse41 with mysql_native_password, COM_QUERY, read the reply. Not a driver.
import { connect } from "cloudflare:sockets";

class Reader {
  buf = new Uint8Array(0);
  r: ReadableStreamDefaultReader<Uint8Array>;
  constructor(r: ReadableStreamDefaultReader<Uint8Array>) {
    this.r = r;
  }
  async packet(): Promise<{ seq: number; body: Uint8Array }> {
    while (true) {
      if (this.buf.length >= 4) {
        const len = this.buf[0] | (this.buf[1] << 8) | (this.buf[2] << 16);
        if (this.buf.length >= 4 + len) {
          const seq = this.buf[3];
          const body = this.buf.slice(4, 4 + len);
          this.buf = this.buf.slice(4 + len);
          return { seq, body };
        }
      }
      const { value, done } = await this.r.read();
      if (done) throw new Error("connection closed");
      const n = new Uint8Array(this.buf.length + value.length);
      n.set(this.buf);
      n.set(value, this.buf.length);
      this.buf = n;
    }
  }
}

function pkt(seq: number, body: Uint8Array) {
  const out = new Uint8Array(4 + body.length);
  out[0] = body.length & 0xff;
  out[1] = (body.length >> 8) & 0xff;
  out[2] = (body.length >> 16) & 0xff;
  out[3] = seq;
  out.set(body, 4);
  return out;
}

const u32 = (n: number) => new Uint8Array([n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff, (n >>> 24) & 0xff]);
const cat = (...a: Uint8Array[]) => {
  const out = new Uint8Array(a.reduce((s, x) => s + x.length, 0));
  let o = 0;
  for (const x of a) out.set(x, o), (o += x.length);
  return out;
};
const sha1 = async (b: Uint8Array) => new Uint8Array(await crypto.subtle.digest("SHA-1", b));

export async function tcpProbe(databaseUrl: string) {
  const u = new URL(databaseUrl);
  const enc = new TextEncoder();
  const steps: Record<string, number> = {};
  const t0 = Date.now();
  const socket = connect({ hostname: u.hostname, port: Number(u.port || 4000) }, { secureTransport: "starttls", allowHalfOpen: false });
  let reader = new Reader(socket.readable.getReader());
  const greet = await reader.packet();
  steps.greeting_ms = Date.now() - t0;
  const g = greet.body;
  let i = 1;
  while (g[i] !== 0) i++;
  const serverVersion = new TextDecoder().decode(g.slice(1, i));
  i += 1 + 4; // NUL + connection id
  const salt1 = g.slice(i, i + 8);
  i += 8 + 1 + 2 + 1 + 2 + 2; // filler, caps low, charset, status, caps high
  const authLen = g[i];
  i += 1 + 10;
  const salt2 = g.slice(i, i + Math.max(13, authLen - 8) - 1);
  const salt = cat(salt1, salt2);
  // CLIENT_LONG_PASSWORD|PROTOCOL_41|SECURE_CONNECTION|PLUGIN_AUTH|SSL|CONNECT_WITH_DB
  const caps = 0x1 | 0x200 | 0x8000 | 0x80000 | 0x800 | 0x8;
  const head = cat(u32(caps), u32(16 * 1024 * 1024), new Uint8Array([33]), new Uint8Array(23));
  let w = socket.writable.getWriter();
  await w.write(pkt(1, head));
  w.releaseLock();
  reader.r.releaseLock(); // startTls needs both streams unlocked
  const tls = socket.startTls();
  steps.tcp_and_ssl_request_ms = Date.now() - t0;
  reader = new Reader(tls.readable.getReader());
  w = tls.writable.getWriter();
  const pw = enc.encode(decodeURIComponent(u.password));
  const h1 = await sha1(pw);
  const h2 = await sha1(cat(salt, await sha1(h1)));
  const scramble = h1.map((b, k) => b ^ h2[k]);
  const user = enc.encode(decodeURIComponent(u.username));
  const dbName = enc.encode(u.pathname.replace(/^\//, "") || "lc0");
  const resp = cat(head, user, new Uint8Array([0]), new Uint8Array([scramble.length]), scramble, dbName, new Uint8Array([0]), enc.encode("mysql_native_password"), new Uint8Array([0]));
  await w.write(pkt(2, resp));
  let auth = await reader.packet();
  let plugin = "mysql_native_password";
  if (auth.body[0] === 0xfe) {
    // AuthSwitchRequest: plugin name NUL, then a fresh nonce.
    let j = 1;
    while (auth.body[j] !== 0) j++;
    plugin = new TextDecoder().decode(auth.body.slice(1, j));
    const nonce = auth.body.slice(j + 1, j + 21);
    let reply: Uint8Array;
    if (plugin === "mysql_native_password") {
      const a = await sha1(pw), b = await sha1(cat(nonce, await sha1(a)));
      reply = a.map((x, k) => x ^ b[k]);
    } else if (plugin === "caching_sha2_password") {
      const s256 = async (x: Uint8Array) => new Uint8Array(await crypto.subtle.digest("SHA-256", x));
      const a = await s256(pw), b = await s256(cat(await s256(a), nonce));
      reply = a.map((x, k) => x ^ b[k]);
    } else if (plugin === "mysql_clear_password") {
      reply = cat(pw, new Uint8Array([0])); // over TLS only
    } else throw new Error("unsupported auth plugin " + plugin);
    await w.write(pkt(auth.seq + 1, reply));
    auth = await reader.packet();
    if (plugin === "caching_sha2_password" && auth.body[0] === 0x01) {
      if (auth.body[1] === 0x04) {
        await w.write(pkt(auth.seq + 1, cat(pw, new Uint8Array([0])))); // full auth, over TLS
      }
      auth = await reader.packet();
    }
  }
  steps.tls_and_auth_ms = Date.now() - t0;
  if (auth.body[0] !== 0x00) {
    const msg = auth.body[0] === 0xff ? new TextDecoder().decode(auth.body.slice(9)) : `auth packet 0x${auth.body[0].toString(16)}`;
    await tls.close();
    return { ok: false, serverVersion, plugin, steps, error: msg.replace(/'[^']*'@'[^']*'/g, "'<user>'@'<host>'").slice(0, 200) };
  }
  const queries: number[] = [];
  for (let q = 0; q < 10; q++) {
    const tq = Date.now();
    await w.write(pkt(0, cat(new Uint8Array([0x03]), enc.encode("SELECT 1"))));
    // column count, column def, (EOF), row, EOF/OK: read until an EOF/OK after the row
    let seen = 0;
    while (true) {
      const p = await reader.packet();
      if (p.body[0] === 0xff) throw new Error("query error");
      if (p.body[0] === 0xfe && p.body.length < 9) {
        seen++;
        if (seen === 2) break;
      }
    }
    queries.push(Date.now() - tq);
  }
  await tls.close();
  return { ok: true, serverVersion, plugin, steps, select1_ms: queries };
}
