// What each Overview card reads. Every loader takes plain dependencies and
// returns plain data, so the cards stay presentational and the tests simple.

import { createClient, type Transport } from '@connectrpc/connect';
import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { createDurableApi, envelope } from '@loams/plugin-durable';
import { collection } from '@loams/proto';
import { useEffect, useState } from 'react';

/** The namespace the cards read: the engine's default (Data Studio's default too). */
export const NAMESPACE = 'default';
/** Promises counted per page; more than this reads as "100+". */
export const PENDING_LIMIT = 100;
/** Links described for their lag; the rest are counted only. */
export const LAG_LINKS = 25;

export type Load<T> =
  | { state: 'loading' }
  | { state: 'ok'; data: T }
  | { state: 'error'; message: string };

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Runs `fn` when `key` changes; a later run (or unmount) discards an earlier answer. */
export function useLoad<T>(fn: () => Promise<T>, key: unknown, enabled = true): Load<T> {
  const [v, setV] = useState<Load<T>>({ state: 'loading' });
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` stands for everything `fn` closes over
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    setV({ state: 'loading' });
    fn().then(
      (data) => live && setV({ state: 'ok', data }),
      (e) => live && setV({ state: 'error', message: message(e) }),
    );
    return () => {
      live = false;
    };
  }, [key, enabled]);
  return v;
}

export interface DataSummary {
  namespace: string;
  collections: number;
}

export async function loadData(transport: Transport): Promise<DataSummary> {
  const client = createClient(collection.CollectionService, transport);
  let count = 0;
  let pageToken = '';
  do {
    const res = await client.listCollections({ namespace: NAMESPACE, pageSize: 100, pageToken });
    count += res.collections.length;
    pageToken = res.nextPageToken;
  } while (pageToken);
  return { namespace: NAMESPACE, collections: count };
}

type Fetch = (url: string, init?: RequestInit) => Promise<Response>;
export interface Net {
  fetch: Fetch;
  baseUrl: string;
}

export interface DurableSummary {
  pending: number;
  /** True when the first page was full: there may be more. */
  more: boolean;
}

export async function loadDurable(net: Net): Promise<DurableSummary> {
  const api = createDurableApi(envelope((...a) => net.fetch(...(a as [string])), net.baseUrl));
  const page = await api.searchPromises({ state: 'pending', limit: PENDING_LIMIT });
  return { pending: page.items.length, more: page.items.length >= PENDING_LIMIT };
}

export interface StreamsSummary {
  streams: number;
  links: number;
  /** The largest lag of any partition of any link, with that link's name. */
  maxLag?: { records: number; link: string };
  unregistered: number;
}

async function getJson<T>(net: Net, path: string): Promise<T> {
  const res = await net.fetch(`${net.baseUrl}${path}`, { credentials: 'include' });
  if (!res.ok) throw new Error(`HTTP ${res.status} on ${path}`);
  return (await res.json()) as T;
}

/** Internal streams and links (`_collection.*`) are plumbing, not the user's. */
const visible = (name: string) => !name.startsWith('_');

export async function loadStreams(net: Net): Promise<StreamsSummary> {
  const ns = `/v1/namespaces/${encodeURIComponent(NAMESPACE)}`;
  const [s, l] = await Promise.all([
    getJson<{ streams?: { name: string }[] }>(net, `${ns}/streams`),
    getJson<{ links?: { name: string; status?: string }[] }>(net, `${ns}/links`),
  ]);
  const links = (l.links ?? []).filter((x) => visible(x.name));
  let maxLag: StreamsSummary['maxLag'];
  const described = await Promise.all(
    links.slice(0, LAG_LINKS).map((x) =>
      getJson<{ lag?: { records: number }[] }>(net, `${ns}/links/${encodeURIComponent(x.name)}`)
        .then((d) => ({ name: x.name, lag: d.lag ?? [] }))
        .catch(() => ({ name: x.name, lag: [] })),
    ),
  );
  for (const d of described) {
    for (const p of d.lag) {
      if (!maxLag || p.records > maxLag.records) maxLag = { records: p.records, link: d.name };
    }
  }
  return {
    streams: (s.streams ?? []).filter((x) => visible(x.name)).length,
    links: links.length,
    maxLag,
    unregistered: links.filter((x) => x.status === 'unregistered').length,
  };
}

export interface ConnectorCounts {
  total: number;
  preview: number;
  planned: number;
}

export async function loadConnectors(desktop: LoamsDesktopApi): Promise<ConnectorCounts> {
  const all = await desktop.connectors.catalog();
  return {
    total: all.length,
    preview: all.filter((c) => c.status === 'preview').length,
    planned: all.filter((c) => c.status === 'planned').length,
  };
}
