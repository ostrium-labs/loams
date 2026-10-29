// The pages' server state (console-ui-stack decision 4): TanStack Query over the
// MarketplaceApi of the nearest agentd transport, keyed per transport so two hosts
// never share answers. Tests give the tree a fake API with `MarketplaceApiProvider`.

import { createConnectQueryKey } from '@connectrpc/connect-query';
import type { SearchListingsParams } from '@loams/agentd-client';
import { useAgentdTransport } from '@loams/console-host/query';
import { HarnessService } from '@loams/proto/agentd';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { createContext, createElement, type ReactNode, useContext, useMemo } from 'react';
import { connectMarketplace, type MarketplaceApi } from './api.js';

const ApiContext = createContext<MarketplaceApi | undefined>(undefined);

/** Gives the tree below an API in place of the agentd transport's (tests, previews). */
export function MarketplaceApiProvider({
  api,
  children,
}: {
  api: MarketplaceApi;
  children: ReactNode;
}) {
  return createElement(ApiContext.Provider, { value: api }, children);
}

const ids = new WeakMap<object, number>();
let next = 1;
function idOf(o: object): number {
  let id = ids.get(o);
  if (id === undefined) {
    id = next++;
    ids.set(o, id);
  }
  return id;
}

/** The API, and a key part that names its transport; undefined while no daemon is reachable. */
export function useApi(): { api: MarketplaceApi; key: number } | undefined {
  const given = useContext(ApiContext);
  const transport = useAgentdTransport();
  return useMemo(() => {
    if (given) return { api: given, key: idOf(given) };
    if (!transport) return undefined;
    return { api: connectMarketplace(transport), key: idOf(transport) };
  }, [given, transport]);
}

type Api = NonNullable<ReturnType<typeof useApi>>;

const ROOT = 'marketplace';

export function useSources(a: Api) {
  return useQuery({ queryKey: [ROOT, a.key, 'sources'], queryFn: () => a.api.listSources() });
}

export function useSearch(a: Api, params: SearchListingsParams) {
  return useQuery({
    queryKey: [ROOT, a.key, 'search', params],
    queryFn: () => a.api.search(params),
  });
}

export function useListing(a: Api, id: string, version: string | null, source: string | null) {
  return useQuery({
    queryKey: [ROOT, a.key, 'listing', id, version, source],
    queryFn: () => a.api.getListing({ id, version, source }),
  });
}

export function usePlan(a: Api, id: string, version: string | null, source: string | null) {
  return useQuery({
    queryKey: [ROOT, a.key, 'plan', id, version, source],
    queryFn: () => a.api.planInstall({ id, version, source }),
    // A plan is what the person consents to: it is read once and refreshed only on
    // request ("Read the new plan"), never behind their back.
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    retry: false,
  });
}

export function useInstalled(a: Api) {
  return useQuery({
    queryKey: [ROOT, a.key, 'installed'],
    queryFn: () => a.api.listInstalled(),
  });
}

export function useUpdates(a: Api) {
  return useQuery({
    queryKey: [ROOT, a.key, 'updates'],
    queryFn: () => a.api.checkUpdates(),
  });
}

/**
 * Forgets every marketplace answer of this transport (after an install, an update…),
 * and Studio's harness list, which an installed or removed harness changes.
 */
export function useRefresh(a: Api) {
  const client = useQueryClient();
  return async (what?: 'sources' | 'installed') => {
    await client.invalidateQueries({ queryKey: what ? [ROOT, a.key, what] : [ROOT, a.key] });
    if (what !== 'sources') {
      await client.invalidateQueries({
        queryKey: createConnectQueryKey({
          schema: HarnessService.method.listHarnesses,
          cardinality: 'finite',
        }),
      });
    }
  };
}
