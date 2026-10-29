// Addresses, ids and tokens (design 78 §25.3, D1739, D1740). Web Crypto only, so
// the same code runs in the Worker and in Node's test runner.

const enc = new TextEncoder();
const dec = new TextDecoder();

export function hex(bytes: ArrayBuffer | Uint8Array): string {
  const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let s = "";
  for (const x of b) s += x.toString(16).padStart(2, "0");
  return s;
}

export function b64url(bytes: Uint8Array): string {
  let s = "";
  for (const x of bytes) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function b64(bytes: Uint8Array): string {
  let s = "";
  for (const x of bytes) s += String.fromCharCode(x);
  return btoa(s);
}

function unb64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function randomBytes(n: number): Uint8Array {
  const b = new Uint8Array(n);
  crypto.getRandomValues(b);
  return b;
}

/** An id: prefix + 128 random bits, base64url (wl_, inv_, acct_, evt_). */
export function newId(prefix: string): string {
  return `${prefix}_${b64url(randomBytes(16))}`;
}

/** A bearer token: prefix + 256 random bits (wlc_ confirm, wlr_ remove; D1740). */
export function newToken(prefix: "wlc" | "wlr"): string {
  return `${prefix}_${b64url(randomBytes(32))}`;
}

export async function sha256Hex(s: string): Promise<string> {
  return hex(await crypto.subtle.digest("SHA-256", enc.encode(s)));
}

/** Constant-time comparison of two strings of the same alphabet. */
export function timingSafeEqual(a: string, b: string): boolean {
  let diff = a.length ^ b.length;
  const n = Math.max(a.length, b.length);
  for (let i = 0; i < n; i++) diff |= (a.charCodeAt(i) || 0) ^ (b.charCodeAt(i) || 0);
  return diff === 0;
}

/**
 * Normalises an address for HMAC and uniqueness: trimmed, the domain
 * lower-cased, and the local part lower-cased too. Every mailbox provider we
 * have seen ignores case; RFC 5321 allows otherwise, but treating Bob@ and bob@
 * as two waitlist entries only helps someone flooding the list. Plus-addresses
 * are kept as given (Q1286): they are real, distinct inboxes.
 */
export function normaliseEmail(raw: string): string | null {
  const s = raw.trim();
  if (s.length < 3 || s.length > 254) return null;
  const at = s.lastIndexOf("@");
  if (at <= 0 || at === s.length - 1) return null;
  const local = s.slice(0, at).toLowerCase();
  const domain = s.slice(at + 1).toLowerCase().replace(/\.$/, "");
  if (local.length > 64) return null;
  if (!/^[a-z0-9!#$%&'*+/=?^_`{|}~.-]+$/.test(local) || local.startsWith(".") || local.endsWith(".") || local.includes("..")) return null;
  if (!/^(?=.{1,253}$)([a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/.test(domain)) return null;
  return `${local}@${domain}`;
}

export function domainOf(normalised: string): string {
  return normalised.slice(normalised.lastIndexOf("@") + 1);
}

/** Matches anything that looks like an address inside free text. */
export const EMAIL_IN_TEXT = /[^\s@<>()",;:]+@[a-z0-9-]+(\.[a-z0-9-]+)+/i;

export interface Keys {
  /** HMAC-SHA-256 key for address and IP-prefix lookups. */
  hmac: CryptoKey;
  /** AES-256-GCM key for the stored addresses. */
  aes: CryptoKey;
}

/** Imports the two Worker secrets, each 32 bytes, base64. */
export async function importKeys(hmacB64: string, aesB64: string): Promise<Keys> {
  const h = unb64(hmacB64);
  const a = unb64(aesB64);
  if (h.length < 32 || a.length !== 32) throw new Error("LOAMS_CLOUD_HMAC_KEY needs at least 32 bytes and LOAMS_CLOUD_AES_KEY exactly 32");
  return {
    hmac: await crypto.subtle.importKey("raw", h as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]),
    aes: await crypto.subtle.importKey("raw", a as BufferSource, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]),
  };
}

/** Fresh random keys, for tests. */
export async function testKeys(): Promise<Keys> {
  return importKeys(b64(randomBytes(32)), b64(randomBytes(32)));
}

export async function hmacHex(keys: Keys, purpose: string, value: string): Promise<string> {
  // The purpose separates the address HMAC from the IP-prefix HMAC under one key.
  return hex(await crypto.subtle.sign("HMAC", keys.hmac, enc.encode(`${purpose}\u0000${value}`)));
}

export function emailHmac(keys: Keys, normalised: string): Promise<string> {
  return hmacHex(keys, "email", normalised);
}

export async function encrypt(keys: Keys, plaintext: string): Promise<string> {
  const iv = randomBytes(12);
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: iv as BufferSource }, keys.aes, enc.encode(plaintext)));
  const out = new Uint8Array(iv.length + ct.length);
  out.set(iv);
  out.set(ct, iv.length);
  return b64(out);
}

export async function decrypt(keys: Keys, stored: string): Promise<string> {
  const b = unb64(stored);
  const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: b.slice(0, 12) as BufferSource }, keys.aes, b.slice(12) as BufferSource);
  return dec.decode(pt);
}

/** The /24 (IPv4) or /48 (IPv6) prefix of a client address, for rate limits (§25.6). */
export function ipPrefix(ip: string): string {
  const v4 = ip.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.\d{1,3}$/);
  if (v4) return `${v4[1]}.${v4[2]}.${v4[3]}.0/24`;
  if (ip.includes(":")) {
    const [head, tail = ""] = ip.toLowerCase().split("::");
    const h = head ? head.split(":") : [];
    const t = tail ? tail.split(":") : [];
    const groups = ip.includes("::") ? [...h, ...Array(8 - h.length - t.length).fill("0"), ...t] : h;
    return `${groups.slice(0, 3).map((g) => g.replace(/^0+(?=.)/, "")).join(":")}::/48`;
  }
  return "unknown";
}
