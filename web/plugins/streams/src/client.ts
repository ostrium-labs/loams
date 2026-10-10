// The streams and links client: native REST over the active server's
// `platform.fetch`. Everything the pages see is plain JSON.

export interface StreamSummary {
  name: string;
  partitions: number;
  retention?: { max_age_ms?: number | null; max_bytes?: number | null };
}

/** Offsets are decimal strings: a u64 offset can exceed 2^53. Do arithmetic with BigInt. */
export interface PartitionBounds {
  partition: number;
  log_start_offset: string;
  high_watermark: string;
}

export interface StreamDetail {
  id: number;
  partitions: PartitionBounds[];
  retention: { max_age_ms?: number | null; max_bytes?: number | null };
}

/** `running` means a target factory is registered, not that the link is live. */
export type LinkStatus = 'running' | 'unregistered';

export interface LinkSummary {
  name: string;
  source: string | null;
  target: { kind: string; name: string };
  status: LinkStatus;
}

export interface LinkDetail {
  id: number;
  name: string;
  source: string;
  target: { kind: string; name: string };
  options: Record<string, string>;
  status: LinkStatus;
  version?: number;
  applied?: { partition: number; offset: string }[];
  lag: { partition: number; records: string }[];
}

export interface TailRecord {
  offset: string;
  key: string | null;
  value: string;
  timestamp_ms: number;
}

export interface FetchPage {
  records: TailRecord[];
  nextOffset: string;
  highWatermark: string;
}

/** The engine's code for a fetch offset outside the log's bounds (HTTP 416). */
export const OFFSET_OUT_OF_RANGE = 'offset_out_of_range';

/** An error from a route, `{error, message}`. */
export class ApiError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/** The JSON number fields that hold a u64: kept as their exact decimal text. */
const U64_KEYS = new Set([
  'offset',
  'next_offset',
  'high_watermark',
  'log_start_offset',
  'base_offset',
  'last_offset',
  'records',
]);

/** Parses JSON, turning the u64 fields into exact decimal strings (no precision loss past 2^53). */
export function parseJsonU64(text: string): unknown {
  return JSON.parse(text, function (
    this: unknown,
    key: string,
    value: unknown,
    ctx?: { source?: string },
  ) {
    if (typeof value === 'number' && U64_KEYS.has(key)) {
      return ctx?.source !== undefined ? ctx.source : String(value);
    }
    return value;
  } as Parameters<typeof JSON.parse>[1]);
}

export const isInternal = (name: string | null | undefined) => (name ?? '').startsWith('_');

export function encodeB64(text: string): string {
  let bin = '';
  for (const b of new TextEncoder().encode(text)) bin += String.fromCharCode(b);
  return btoa(bin);
}

export function decodeB64(b64: string | null | undefined): string {
  if (b64 == null) return '';
  try {
    const bin = atob(b64);
    const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
    return new TextDecoder('utf-8', { fatal: false }).decode(bytes);
  } catch {
    return b64;
  }
}

export function createStreamsClient(fetchFn: typeof globalThis.fetch, baseUrl = '') {
  const enc = encodeURIComponent;
  const root = (ns: string) => `${baseUrl}/v1/namespaces/${enc(ns)}`;

  async function call<T>(url: string, init?: RequestInit): Promise<T> {
    const res = await fetchFn(url, { ...init, credentials: 'include' });
    const text = await res.text().catch(() => '');
    let body: Record<string, unknown> = {};
    try {
      body = parseJsonU64(text) as Record<string, unknown>;
    } catch {
      // a non-JSON body: the status text stands in
    }
    if (!res.ok) {
      throw new ApiError(
        String(body.error ?? body.code ?? 'error'),
        String(body.message ?? res.statusText),
        res.status,
      );
    }
    return body as T;
  }
  const post = <T>(url: string, body: unknown, contentType = 'application/json') =>
    call<T>(url, {
      method: 'POST',
      headers: { 'content-type': contentType },
      body: typeof body === 'string' ? body : JSON.stringify(body),
    });

  return {
    async listStreams(ns: string): Promise<StreamSummary[]> {
      return (await call<{ streams: StreamSummary[] }>(`${root(ns)}/streams`)).streams;
    },
    describeStream(ns: string, stream: string): Promise<StreamDetail> {
      return call(`${root(ns)}/streams/${enc(stream)}`);
    },
    async createStream(
      ns: string,
      spec: {
        name: string;
        partitions: number;
        retention?: { max_age_ms?: number; max_bytes?: number };
      },
    ): Promise<void> {
      await post(`${root(ns)}/streams`, spec);
    },
    /** Produces one record (value and key as text) to a partition. */
    produce(
      ns: string,
      stream: string,
      partition: number,
      rec: { key?: string; value: string },
    ): Promise<{ base_offset: string; last_offset: string }> {
      return post(`${root(ns)}/streams/${enc(stream)}/partitions/${partition}/records`, {
        records: [
          {
            ...(rec.key ? { key: encodeB64(rec.key) } : {}),
            value: encodeB64(rec.value),
          },
        ],
      });
    },
    /** Produces one structured CloudEvent. */
    produceEvent(
      ns: string,
      stream: string,
      partition: number,
      event: Record<string, unknown>,
    ): Promise<{ events: { status: string; partition?: number; offset?: string }[] }> {
      return post(
        `${root(ns)}/streams/${enc(stream)}/events?partition=${partition}`,
        event,
        'application/cloudevents+json',
      );
    },
    async fetch(ns: string, stream: string, partition: number, offset: string): Promise<FetchPage> {
      if (!/^\d+$/.test(offset)) throw new Error(`Bad offset ${offset}.`);
      const res = await call<{
        records: {
          offset: string;
          key: string | null;
          value: string | null;
          timestamp_ms: number;
        }[];
        next_offset: string;
        high_watermark: string;
      }>(
        `${root(ns)}/streams/${enc(stream)}/partitions/${partition}/records?offset=${offset}&max_bytes=262144`,
      );
      return {
        records: res.records.map((r) => ({
          offset: r.offset,
          key: r.key == null ? null : decodeB64(r.key),
          value: decodeB64(r.value),
          timestamp_ms: r.timestamp_ms,
        })),
        nextOffset: res.next_offset,
        highWatermark: res.high_watermark,
      };
    },
    async listLinks(ns: string): Promise<LinkSummary[]> {
      return (await call<{ links: LinkSummary[] }>(`${root(ns)}/links`)).links;
    },
    describeLink(ns: string, link: string): Promise<LinkDetail> {
      return call(`${root(ns)}/links/${enc(link)}`);
    },
    async createLink(
      ns: string,
      spec: {
        name: string;
        source: string;
        target: { kind: string; name: string };
        options: Record<string, string>;
      },
    ): Promise<void> {
      await post(`${root(ns)}/links`, spec);
    },
  };
}

export type StreamsClient = ReturnType<typeof createStreamsClient>;
