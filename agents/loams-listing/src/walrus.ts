// Walrus over plain HTTP for the publish tooling (plan MS1 Task 9, D1812):
// store a blob through a publisher, read it back through an aggregator, and
// keep the epochs a store reports in the walrus record beside the listing.
// The endpoints are the ones crates/loams-walrus/README.md lists (checked
// 2026-10-25). There is no default publisher (ruling 1 of Task 9).
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';

export type Network = 'testnet' | 'mainnet';

/** An epoch's length: 1 day on Testnet, 2 weeks on Mainnet. */
export const EPOCH_MS: Record<Network, number> = {
  testnet: 24 * 60 * 60 * 1000,
  mainnet: 14 * 24 * 60 * 60 * 1000,
};

/** Most public publishers and aggregators cap a request at 10 MiB. */
export const DEFAULT_MAX_BYTES = 10 * 1024 * 1024;
/** Storage can be bought at most 53 epochs ahead. */
export const MAX_EPOCHS = 53;
export const DEFAULT_EPOCHS = 5;
/** A 404 this soon after our own store is retried (as loams-walrus does). */
export const DEFAULT_READ_AFTER_WRITE_MS = 180_000;

export class WalrusError extends Error {}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex');
}

/** `https://`, or `http://` on loopback only (the tests' fakes). */
export function checkEndpoint(name: string, raw: string): URL {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new WalrusError(`${name} ${raw} is not a URL`);
  }
  const loopback = ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname);
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) {
    throw new WalrusError(`${name} ${raw} must be https:// (http:// only on loopback)`);
  }
  return url;
}

function endpoint(base: URL, path: string): URL {
  const root = base.href.endsWith('/') ? base.href : `${base.href}/`;
  return new URL(path, root);
}

export interface StoreResult {
  blobId: string;
  endEpoch: number;
  /** `true` for `newlyCreated`, `false` for `alreadyCertified`. */
  newlyCreated: boolean;
  /** The epoch the blob was registered in (a new blob only). */
  registeredEpoch?: number;
}

function num(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

/** Parses a publisher's answer to `PUT /v1/blobs`. */
export function parseStoreAnswer(answer: unknown): StoreResult {
  const a = answer as {
    newlyCreated?: { blobObject?: { blobId?: unknown; registeredEpoch?: unknown; storage?: { endEpoch?: unknown } } };
    alreadyCertified?: { blobId?: unknown; endEpoch?: unknown };
  } | null;
  const created = a?.newlyCreated?.blobObject;
  if (created) {
    const endEpoch = num(created.storage?.endEpoch);
    if (typeof created.blobId !== 'string' || endEpoch === undefined) {
      throw new WalrusError('the publisher answered newlyCreated without a blobId and storage.endEpoch');
    }
    return { blobId: created.blobId, endEpoch, newlyCreated: true, registeredEpoch: num(created.registeredEpoch) };
  }
  const certified = a?.alreadyCertified;
  if (certified) {
    const endEpoch = num(certified.endEpoch);
    if (typeof certified.blobId !== 'string' || endEpoch === undefined) {
      throw new WalrusError('the publisher answered alreadyCertified without a blobId and endEpoch');
    }
    return { blobId: certified.blobId, endEpoch, newlyCreated: false };
  }
  throw new WalrusError(`the publisher did not store the blob: ${JSON.stringify(answer).slice(0, 200)}`);
}

export interface PublisherOptions {
  publisher: URL;
  /** Sent as `Authorization: Bearer <token>`. */
  token?: string;
  epochs: number;
  permanent: boolean;
}

/** `PUT <publisher>/v1/blobs?epochs=<n>[&permanent=true]`. */
export async function storeBlob(bytes: Uint8Array, options: PublisherOptions): Promise<StoreResult> {
  const url = endpoint(options.publisher, 'v1/blobs');
  url.searchParams.set('epochs', String(options.epochs));
  if (options.permanent) url.searchParams.set('permanent', 'true');
  const headers: Record<string, string> = { 'content-type': 'application/octet-stream', 'user-agent': 'loams-listing' };
  if (options.token) headers.authorization = `Bearer ${options.token}`;
  const response = await fetch(url, { method: 'PUT', headers, body: bytes });
  const text = await response.text();
  if (!response.ok) throw new WalrusError(`${url.origin}: the publisher answered HTTP ${response.status}: ${text.slice(0, 200)}`);
  let answer: unknown;
  try {
    answer = JSON.parse(text);
  } catch {
    throw new WalrusError(`${url.origin}: the publisher's answer is not JSON`);
  }
  return parseStoreAnswer(answer);
}

export interface ReadOptions {
  aggregator: URL;
  /** Retry a 404 for this long (a CDN can cache one just after certification). */
  retry404Ms: number;
  maxBytes: number;
  /** For tests: the first backoff. */
  backoffMs?: number;
}

/** `GET <aggregator>/v1/blobs/<id>`, retrying a 404 with doubling backoff capped at 10 s. Unverified. */
export async function readBlob(blobId: string, options: ReadOptions): Promise<Uint8Array> {
  const url = endpoint(options.aggregator, `v1/blobs/${encodeURIComponent(blobId)}`);
  const deadline = Date.now() + options.retry404Ms;
  let backoff = options.backoffMs ?? 500;
  for (;;) {
    const response = await fetch(url, { headers: { 'user-agent': 'loams-listing' } });
    if (response.status === 404 && Date.now() + backoff <= deadline) {
      await response.body?.cancel();
      await new Promise((r) => setTimeout(r, backoff));
      backoff = Math.min(backoff * 2, 10_000);
      continue;
    }
    if (!response.ok) {
      await response.body?.cancel();
      throw new WalrusError(`${url.origin}: reading ${blobId} answered HTTP ${response.status}`);
    }
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length > options.maxBytes) throw new WalrusError(`${blobId}: over the ${options.maxBytes}-byte cap`);
    return bytes;
  }
}

/** Reads a blob and checks its SHA-256; a mismatch is an error and the bytes are dropped. */
export async function readVerified(blobId: string, sha256: string, options: ReadOptions): Promise<Uint8Array> {
  const bytes = await readBlob(blobId, options);
  const actual = sha256Hex(bytes);
  if (actual !== sha256) throw new WalrusError(`${blobId}: the aggregator sent bytes with sha256 ${actual}, not ${sha256}`);
  return bytes;
}

// ---- the walrus record (ruling 2 of Task 9) ----

export const RECORD_SCHEMA = 'loams.listing.walrus.v1';

export interface RecordedBlob {
  blob_id: string;
  sha256: string;
  /** The epoch the blob is stored until (exclusive). */
  end_epoch: number;
  /** When it was stored (RFC 3339). */
  stored_at: string;
  /** The network's epoch at `stored_at`, when the publisher's answer told it. */
  epoch_at_store?: number;
}

export interface WalrusRecord {
  schema: typeof RECORD_SCHEMA;
  network: Network;
  blobs: RecordedBlob[];
}

/** `listing.json` → `listing.walrus.json`. */
export function recordPathFor(listingPath: string): string {
  return listingPath.endsWith('.json') ? `${listingPath.slice(0, -5)}.walrus.json` : `${listingPath}.walrus.json`;
}

export function readRecord(path: string): WalrusRecord | undefined {
  if (!existsSync(path)) return undefined;
  const record = JSON.parse(readFileSync(path, 'utf8')) as WalrusRecord;
  if (record.schema !== RECORD_SCHEMA || !Array.isArray(record.blobs) || !(record.network in EPOCH_MS)) {
    throw new WalrusError(`${path}: not a ${RECORD_SCHEMA} record`);
  }
  return record;
}

export function writeRecord(path: string, record: WalrusRecord): void {
  writeFileSync(path, `${JSON.stringify(record, null, 2)}\n`);
}

/** Adds or replaces (by blob id) a blob in the record. */
export function recordBlob(record: WalrusRecord | undefined, network: Network, blob: RecordedBlob): WalrusRecord {
  if (record && record.network !== network) {
    throw new WalrusError(`the walrus record is for ${record.network}, not ${network}`);
  }
  const blobs = (record?.blobs ?? []).filter((b) => b.blob_id !== blob.blob_id);
  blobs.push(blob);
  return { schema: RECORD_SCHEMA, network, blobs };
}

/**
 * The network's current epoch, estimated from the newest store whose epoch is known plus
 * the whole epochs since. `undefined` when no store in the record knows its epoch.
 */
export function estimateEpoch(record: WalrusRecord, now: Date): number | undefined {
  const anchors = record.blobs
    .filter((b) => b.epoch_at_store !== undefined && !Number.isNaN(Date.parse(b.stored_at)))
    .sort((a, b) => Date.parse(b.stored_at) - Date.parse(a.stored_at));
  const anchor = anchors[0];
  if (!anchor) return undefined;
  const elapsed = Math.max(0, now.getTime() - Date.parse(anchor.stored_at));
  return anchor.epoch_at_store! + Math.floor(elapsed / EPOCH_MS[record.network]);
}

export type EpochStatus =
  | { state: 'ok'; endEpoch: number; current: number }
  | { state: 'expiring'; endEpoch: number; current: number }
  | { state: 'expired'; endEpoch: number; current: number };

/** Q1332: within `window` epochs of its end is a warning; at or past it, expired. */
export function epochStatus(endEpoch: number, current: number, window: number): EpochStatus {
  if (endEpoch <= current) return { state: 'expired', endEpoch, current };
  if (endEpoch < current + window) return { state: 'expiring', endEpoch, current };
  return { state: 'ok', endEpoch, current };
}
