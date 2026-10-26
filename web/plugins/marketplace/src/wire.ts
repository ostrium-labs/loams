// The Seed Exchange's two wire forms as one value (plan MK1 Task 4b, ruling 3).
//
// The pages are written against the ndjson types in @loams/agentd-client: plain JSON
// with the listing schema's strings (`mcp-server`, `mastra`, `core`, `destructive`)
// and the listing's parts (its Place, models, cost, provenance, the listing itself)
// as listing JSON. The console reaches agentd over Connect, whose
// `loams.agentd.v1.MarketplaceService` carries `loams.marketplace.v1` messages and
// enums. This file maps between them the way loams-agentd-connect's
// `map/marketplace.rs` does: an enum value is the schema string upper-cased, `-` as
// `_`, after the enum's prefix (`"mcp-server"` is `KIND_MCP_SERVER`); a listing part
// is its proto3 JSON under the proto field names, with the enum strings renamed.

import {
  type DescEnum,
  type DescField,
  type DescMessage,
  type JsonObject,
  type JsonValue,
  type MessageShape,
  ScalarType,
  toJson,
} from '@bufbuild/protobuf';
import type {
  AvailableUpdate,
  InstalledListing,
  InstallPlan,
  ListingDetail,
  ListingRevocation,
  ListingSummary,
  ListingTier,
  ListingVersion,
  MarketplaceSource,
  SearchListingsParams,
} from '@loams/agentd-client';
import * as pb from '@loams/proto/agentd';
import * as listing from '@loams/proto/marketplace';

/** `FrameworkName` → `FRAMEWORK_NAME_`, as the Rust mapping names a prefix. */
export function prefixOf(enumName: string): string {
  return `${enumName.replace(/(?!^)([A-Z])/g, '_$1').toUpperCase()}_`;
}

/** A proto enum value's name as its schema string; unspecified (or unknown) is undefined. */
export function enumString(desc: DescEnum, value: number | string): string | undefined {
  const name = typeof value === 'string' ? value : desc.value[value]?.name;
  const prefix = prefixOf(desc.name);
  if (!name?.startsWith(prefix)) return undefined;
  const rest = name.slice(prefix.length);
  if (rest === 'UNSPECIFIED') return undefined;
  // The schema's own spelling: a platform keeps its architecture's underscore
  // (`linux-x86_64`), as map/marketplace.rs keeps it.
  if (desc.name === 'Platform') return rest.toLowerCase().replace('_', '-');
  return rest.toLowerCase().replace(/_/g, '-');
}

/** A schema string as a proto enum number; an unknown string is 0 (unspecified). */
export function enumNumber(desc: DescEnum, value: string): number {
  const name = `${prefixOf(desc.name)}${value.toUpperCase().replace(/-/g, '_')}`;
  // (`linux-x86_64` is PLATFORM_LINUX_X86_64 either way.)
  return desc.values.find((v) => v.name === name)?.number ?? 0;
}

const INT64 = new Set([
  ScalarType.INT64,
  ScalarType.UINT64,
  ScalarType.SINT64,
  ScalarType.FIXED64,
  ScalarType.SFIXED64,
]);

function scalar(type: ScalarType, value: JsonValue): JsonValue {
  return INT64.has(type) && typeof value === 'string' ? Number(value) : value;
}

function renamed(desc: DescEnum, value: JsonValue): JsonValue {
  return typeof value === 'string' || typeof value === 'number'
    ? (enumString(desc, value) ?? value)
    : value;
}

function rewriteField(field: DescField, value: JsonValue): JsonValue {
  switch (field.fieldKind) {
    case 'scalar':
      return scalar(field.scalar, value);
    case 'enum':
      return renamed(field.enum, value);
    case 'message':
      return rewrite(field.message, value);
    case 'list':
      if (!Array.isArray(value)) return value;
      if (field.listKind === 'enum') return value.map((v) => renamed(field.enum, v));
      if (field.listKind === 'message') return value.map((v) => rewrite(field.message, v));
      return value.map((v) => scalar(field.scalar, v));
    case 'map': {
      if (value === null || typeof value !== 'object' || Array.isArray(value)) return value;
      const out: JsonObject = {};
      for (const [k, v] of Object.entries(value)) {
        out[k] =
          field.mapKind === 'enum'
            ? renamed(field.enum, v)
            : field.mapKind === 'message'
              ? rewrite(field.message, v)
              : scalar(field.scalar, v);
      }
      return out;
    }
  }
}

/** Walks proto3 JSON (proto field names) beside its descriptor, renaming enums. */
function rewrite(desc: DescMessage, value: JsonValue): JsonValue {
  // A Struct (an A2A agent card) is JSON already.
  if (desc.typeName.startsWith('google.protobuf.')) return value;
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return value;
  const out: JsonObject = {};
  for (const [key, v] of Object.entries(value)) {
    const field = desc.fields.find((f) => f.name === key);
    out[key] = field ? rewriteField(field, v) : v;
  }
  return out;
}

/** A `loams.marketplace.v1` message as the listing JSON the ndjson wire carries. */
export function partJson<D extends DescMessage>(
  desc: D,
  message: MessageShape<D> | undefined,
): Record<string, unknown> | null {
  if (!message) return null;
  const json = toJson(desc, message, { useProtoFieldName: true }) as JsonValue;
  return rewrite(desc, json) as Record<string, unknown>;
}

// ── loams.agentd.v1 → the ndjson types ─────────────────────────────────────────

const TIERS: Record<number, ListingTier> = {
  [pb.ListingTier.FIRST_PARTY]: 'firstParty',
  [pb.ListingTier.VERIFIED]: 'verified',
  [pb.ListingTier.COMMUNITY]: 'community',
};

/** The ndjson tier as the proto enum. */
export function tierNumber(tier: ListingTier): pb.ListingTier {
  return tier === 'firstParty'
    ? pb.ListingTier.FIRST_PARTY
    : tier === 'verified'
      ? pb.ListingTier.VERIFIED
      : pb.ListingTier.COMMUNITY;
}

const orNull = (s: string): string | null => (s === '' ? null : s);

export function revocationOf(r: pb.Revocation | undefined): ListingRevocation | null {
  if (!r) return null;
  return {
    severity: r.severity === pb.RevocationSeverity.DISABLE ? 'disable' : 'warn',
    reason: r.reason,
    versions: r.versions,
    advisory: orNull(r.advisory),
  };
}

export function summaryOf(s: pb.ListingSummary | undefined): ListingSummary {
  const m = s ?? undefined;
  return {
    id: m?.id ?? '',
    source: m?.source ?? '',
    kind: (m && enumString(listing.KindSchema, m.kind)) ?? '',
    title: m?.title ?? '',
    summary: m?.summary ?? '',
    // An unspecified tier is shown as the least trusted one, never as first-party.
    tier: (m && TIERS[m.tier]) ?? 'community',
    latest: m ? orNull(m.latest) : null,
    framework: (m && enumString(listing.FrameworkNameSchema, m.framework)) ?? null,
    language: (m && enumString(listing.LanguageSchema, m.language)) ?? null,
    level: (m && enumString(listing.ConformanceLevelSchema, m.level)) ?? null,
    revocation: revocationOf(m?.revocation),
  };
}

export function sourceOf(s: pb.MarketplaceSource): MarketplaceSource {
  return {
    name: s.name,
    url: s.url,
    identity: s.identity,
    issuer: s.issuer,
    official: s.official,
    refreshedMs: Number(s.refreshedMs),
    error: orNull(s.error),
    listings: s.listings,
  };
}

function versionOf(v: pb.ListingVersion): ListingVersion {
  return { version: v.version, yanked: orNull(v.yanked), revocation: revocationOf(v.revocation) };
}

export function detailOf(r: pb.GetListingResponse): ListingDetail {
  return {
    summary: summaryOf(r.summary),
    listing: partJson(listing.ListingSchema, r.listing) ?? {},
    versions: r.versions.map(versionOf),
  };
}

export function planOf(p: pb.InstallPlan | undefined): InstallPlan {
  if (!p) throw new Error('agentd answered PlanInstall without a plan');
  return {
    digest: p.digest,
    summary: summaryOf(p.summary),
    version: p.version,
    place: partJson(listing.PlaceTemplateSchema, p.place),
    tools: p.tools.map((t) => ({
      name: t.name,
      description: t.description,
      risk: enumString(listing.RiskSchema, t.risk) ?? '',
      alwaysAsks: t.alwaysAsks,
      mcpServer: orNull(t.mcpServer),
    })),
    egress: [...p.egress],
    dependencies: p.dependencies.map((d) => ({
      id: d.id,
      version: d.version,
      installed: d.installed,
    })),
    secrets: p.secrets.map((s) => ({
      name: s.name,
      description: s.description,
      required: s.required,
    })),
    models: partJson(listing.ModelsSchema, p.models),
    cost: partJson(listing.CostProfileSchema, p.cost),
    licence: p.licence,
    provenance: partJson(listing.ProvenanceSchema, p.provenance),
    sourceRepository: p.sourceRepository,
    // Ruling 10 of Task 4b: unset (0) while the index carries no publish date.
    versionAgeDays: p.versionAgeDays > 0 ? p.versionAgeDays : null,
    cannot: [...p.cannot],
  };
}

export function installedOf(i: pb.InstalledListing | undefined): InstalledListing {
  if (!i) throw new Error('agentd answered without the installed listing');
  return {
    id: i.id,
    version: i.version,
    source: i.source,
    kind: enumString(listing.KindSchema, i.kind) ?? '',
    installedMs: Number(i.installedMs),
    revocation: revocationOf(i.revocation),
    update: orNull(i.update),
  };
}

export function updateOf(u: pb.AvailableUpdate): AvailableUpdate {
  return { id: u.id, from: u.from, to: u.to, widens: u.widens };
}

/** The ndjson search parameters as the Connect request's fields. */
export function searchRequest(p: SearchListingsParams) {
  return {
    query: p.query,
    kinds: p.kinds.map((k) => enumNumber(listing.KindSchema, k)),
    frameworks: p.frameworks.map((f) => enumNumber(listing.FrameworkNameSchema, f)),
    languages: p.languages.map((l) => enumNumber(listing.LanguageSchema, l)),
    minLevel: p.minLevel ? enumNumber(listing.ConformanceLevelSchema, p.minLevel) : 0,
    tiers: p.tiers.map(tierNumber),
    source: p.source ?? '',
  };
}

/** The schema strings of an enum, in declaration order (the browse filters). */
export function enumStrings(desc: DescEnum): string[] {
  return desc.values.map((v) => enumString(desc, v.number)).filter((s): s is string => !!s);
}
