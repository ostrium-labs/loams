// What the Seed Exchange's pages ask of agentd (design §73 §7.1–§7.2; plan MK1 Tasks
// 4b and 9): `MarketplaceService` over the console's agentd transport, answered in
// the ndjson types of @loams/agentd-client (wire.ts). Tests give the pages a fake.

import { createClient, type Transport } from '@connectrpc/connect';
import type {
  AddMarketplaceSourceParams,
  AvailableUpdate,
  InstalledListing,
  InstallListingParams,
  InstallPlan,
  ListingDetail,
  ListingRef,
  ListingSummary,
  MarketplaceSource,
  SearchListingsParams,
  UpdateListingParams,
} from '@loams/agentd-client';
import { MarketplaceService } from '@loams/proto/agentd';
import {
  detailOf,
  installedOf,
  planOf,
  searchRequest,
  sourceOf,
  summaryOf,
  updateOf,
} from './wire.js';

export interface MarketplaceApi {
  listSources(): Promise<MarketplaceSource[]>;
  addSource(params: AddMarketplaceSourceParams): Promise<MarketplaceSource>;
  removeSource(name: string): Promise<void>;
  search(params: SearchListingsParams): Promise<ListingSummary[]>;
  getListing(ref: ListingRef): Promise<ListingDetail>;
  /** The consent plan; creates nothing (D1466). */
  planInstall(ref: ListingRef): Promise<InstallPlan>;
  /** Installs exactly the plan whose digest is `params.planDigest`. */
  install(params: InstallListingParams): Promise<InstalledListing>;
  uninstall(id: string): Promise<void>;
  listInstalled(): Promise<InstalledListing[]>;
  checkUpdates(): Promise<AvailableUpdate[]>;
  update(params: UpdateListingParams): Promise<InstalledListing>;
}

/** The Seed Exchange over agentd's Connect transport. */
export function connectMarketplace(transport: Transport): MarketplaceApi {
  const client = createClient(MarketplaceService, transport);
  return {
    async listSources() {
      return (await client.listSources({})).sources.map(sourceOf);
    },
    async addSource(p) {
      const r = await client.addSource({
        name: p.name,
        url: p.url,
        identity: p.identity,
        issuer: p.issuer ?? '',
      });
      if (!r.source) throw new Error('agentd answered AddSource without the source');
      return sourceOf(r.source);
    },
    async removeSource(name) {
      await client.removeSource({ name });
    },
    async search(p) {
      return (await client.searchListings(searchRequest(p))).results.map((s) => summaryOf(s));
    },
    async getListing(ref) {
      return detailOf(
        await client.getListing({
          id: ref.id,
          version: ref.version ?? '',
          source: ref.source ?? '',
        }),
      );
    },
    async planInstall(ref) {
      const r = await client.planInstall({
        id: ref.id,
        version: ref.version ?? '',
        source: ref.source ?? '',
      });
      return planOf(r.plan);
    },
    async install(p) {
      const r = await client.install({
        id: p.id,
        version: p.version ?? '',
        source: p.source ?? '',
        planDigest: p.planDigest,
        config: p.config,
        secrets: p.secrets,
      });
      return installedOf(r.installed);
    },
    async uninstall(id) {
      await client.uninstall({ id });
    },
    async listInstalled() {
      return (await client.listInstalled({})).installed.map((i) => installedOf(i));
    },
    async checkUpdates() {
      return (await client.checkUpdates({})).updates.map(updateOf);
    },
    async update(p) {
      const r = await client.update({
        id: p.id,
        version: p.version ?? '',
        planDigest: p.planDigest ?? '',
      });
      return installedOf(r.installed);
    },
  };
}
