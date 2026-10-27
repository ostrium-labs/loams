// The Seed Exchange's tests: agentd's own wire fixtures (crates/loams-agentd-proto/
// fixtures, which the ndjson round-trip tests keep true) and a fake MarketplaceApi.

import type {
  AvailableUpdateList,
  InstalledListingList,
  InstallListingParams,
  InstallPlan,
  ListingDetail,
  ListingSearchResults,
  MarketplaceSourceList,
  PlanInstallReply,
  SearchListingsParams,
  UpdateListingParams,
} from '@loams/agentd-client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import updatesJson from '../../../../crates/loams-agentd-proto/fixtures/AvailableUpdateList.json';
import installedJson from '../../../../crates/loams-agentd-proto/fixtures/InstalledListingList.json';
import detailJson from '../../../../crates/loams-agentd-proto/fixtures/ListingDetail.json';
import searchJson from '../../../../crates/loams-agentd-proto/fixtures/ListingSearchResults.json';
import sourcesJson from '../../../../crates/loams-agentd-proto/fixtures/MarketplaceSourceList.json';
import planJson from '../../../../crates/loams-agentd-proto/fixtures/PlanInstallReply.json';
import type { MarketplaceApi } from '../src/api.js';
import { MarketplaceApiProvider } from '../src/data.js';

const copy = <T,>(v: T): T => structuredClone(v);

export const PLAN = (planJson as PlanInstallReply).plan;
export const SEARCH = searchJson as ListingSearchResults;
export const DETAIL = detailJson as unknown as ListingDetail;
export const INSTALLED = installedJson as InstalledListingList;
export const UPDATES = updatesJson as AvailableUpdateList;
export const SOURCES = sourcesJson as MarketplaceSourceList;

export interface Calls {
  search: SearchListingsParams[];
  plans: number;
  installs: InstallListingParams[];
  updates: UpdateListingParams[];
  uninstalls: string[];
  added: string[];
  removed: string[];
}

export function fakeApi(
  o: { plan?: InstallPlan; install?: () => Promise<never> | undefined } = {},
) {
  const calls: Calls = {
    search: [],
    plans: 0,
    installs: [],
    updates: [],
    uninstalls: [],
    added: [],
    removed: [],
  };
  const installed = copy(INSTALLED.installed);
  const api: MarketplaceApi = {
    listSources: async () => copy(SOURCES.sources),
    addSource: async (p) => {
      calls.added.push(p.name);
      return {
        ...p,
        issuer: p.issuer ?? '',
        official: false,
        refreshedMs: 0,
        error: null,
        listings: 0,
      };
    },
    removeSource: async (name) => {
      calls.removed.push(name);
    },
    search: async (p) => {
      calls.search.push(p);
      return copy(SEARCH.results);
    },
    getListing: async () => copy(DETAIL),
    planInstall: async () => {
      calls.plans += 1;
      return copy(o.plan ?? PLAN);
    },
    install: async (p) => {
      calls.installs.push(p);
      const failure = o.install?.();
      if (failure) return failure;
      const done = {
        id: p.id,
        version: p.version ?? '',
        source: p.source ?? '',
        kind: 'mcp-server',
        installedMs: 0,
        revocation: null,
        update: null,
      };
      installed.push(done);
      return done;
    },
    uninstall: async (id) => {
      calls.uninstalls.push(id);
    },
    listInstalled: async () => copy(installed),
    checkUpdates: async () => copy(UPDATES.updates),
    update: async (p) => {
      calls.updates.push(p);
      return { ...copy(INSTALLED.installed[0]), version: p.version ?? '' } as never;
    },
  };
  return { api, calls };
}

export function wrap(api: MarketplaceApi) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <MarketplaceApiProvider api={api}>{children}</MarketplaceApiProvider>
    </QueryClientProvider>
  );
}
