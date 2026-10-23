// `loams-listing index build <dir> --out <dir>`: the index repository's
// listings become one deterministic index.json and revocations.json (design
// 73 section 5, D1460). The publish workflow signs both files; this tool never
// signs and never fetches anything.
//
// Input layout (the loams-marketplace repository):
//   listings/<namespace>/<name>/versions/<version>/listing.json
//   listings/<namespace>/<name>/versions/<version>/yanked.json   {"reason": "..."}, optional
//   revocations.json                                             optional
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { FIRST_PARTY_NAMESPACES, formatIssue, hasErrors, lint, type LintOptions } from './lint.js';
import { validateAgainst } from './schema.js';
import { compare, isPrerelease, isRange } from './semver.js';
import type { Listing } from './types.js';

export const INDEX_SCHEMA_ID = 'https://loams.dev/schemas/index/v1.json';
export const REVOCATIONS_SCHEMA_ID = 'https://loams.dev/schemas/revocations/v1.json';

export class IndexError extends Error {
  constructor(public readonly problems: string[]) {
    super(problems.join('\n'));
  }
}

/** JSON with object keys sorted at every level: the same data always gives the same bytes. */
export function canonical(value: unknown): string {
  return `${JSON.stringify(sortKeys(value), null, 2)}\n`;
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value).sort()) out[key] = sortKeys((value as Record<string, unknown>)[key]);
    return out;
  }
  return value;
}

function sha256(text: string): string {
  return createHash('sha256').update(text).digest('hex');
}

function dirs(path: string): string[] {
  if (!existsSync(path)) return [];
  return readdirSync(path)
    .filter((name) => !name.startsWith('.') && statSync(join(path, name)).isDirectory())
    .sort();
}

export interface IndexVersion {
  version: string;
  yanked?: string;
  listing_sha256: string;
  listing: Listing;
}

export interface IndexEntry {
  id: string;
  kind: Listing['kind'];
  title: string;
  summary: string;
  tier: 'first-party' | 'verified' | 'community';
  latest?: string;
  versions: IndexVersion[];
}

export interface Index {
  $schema: string;
  index_version: 1;
  source_commit?: string;
  listings: IndexEntry[];
}

export interface BuildOptions extends LintOptions {
  /** The index repository's commit, recorded in index.json. */
  sourceCommit?: string;
}

export interface BuildResult {
  index: Index;
  revocations: { $schema: string; revocations: unknown[] };
  warnings: string[];
}

function latestOf(versions: IndexVersion[]): string | undefined {
  const live = versions.filter((v) => v.yanked === undefined).map((v) => v.version);
  const stable = live.filter((v) => !isPrerelease(v));
  return (stable.length > 0 ? stable : live).at(-1);
}

export function buildIndex(root: string, options: BuildOptions = {}): BuildResult {
  const problems: string[] = [];
  const warnings: string[] = [];
  const entries: IndexEntry[] = [];
  const listingsDir = join(root, 'listings');
  if (!existsSync(listingsDir)) throw new IndexError([`${listingsDir} does not exist`]);

  for (const namespace of dirs(listingsDir)) {
    for (const name of dirs(join(listingsDir, namespace))) {
      const id = `${namespace}/${name}`;
      const versions: IndexVersion[] = [];
      for (const version of dirs(join(listingsDir, namespace, name, 'versions'))) {
        const dir = join(listingsDir, namespace, name, 'versions', version);
        const file = join(dir, 'listing.json');
        const where = `listings/${id}/versions/${version}/listing.json`;
        if (!existsSync(file)) {
          problems.push(`${where}: missing`);
          continue;
        }
        let listing: Listing;
        try {
          listing = JSON.parse(readFileSync(file, 'utf8')) as Listing;
        } catch (error) {
          problems.push(`${where}: not JSON (${(error as Error).message})`);
          continue;
        }
        const issues = lint(listing, options);
        for (const issue of issues) (issue.severity === 'error' ? problems : warnings).push(formatIssue(where, issue));
        if (hasErrors(issues)) continue;
        if (listing.id !== id) problems.push(`${where}: id ${listing.id} is not ${id}, its directory`);
        if (listing.version !== version) problems.push(`${where}: version ${listing.version} is not ${version}, its directory`);
        let yanked: string | undefined;
        const yankedFile = join(dir, 'yanked.json');
        if (existsSync(yankedFile)) {
          const reason = (JSON.parse(readFileSync(yankedFile, 'utf8')) as { reason?: unknown }).reason;
          if (typeof reason !== 'string' || reason.length === 0) problems.push(`${where}: yanked.json needs a reason`);
          else yanked = reason;
        }
        const text = canonical(listing);
        versions.push({
          version,
          ...(yanked !== undefined ? { yanked } : {}),
          listing_sha256: sha256(text),
          listing: JSON.parse(text) as Listing,
        });
      }
      if (versions.length === 0) {
        problems.push(`listings/${id}: no versions`);
        continue;
      }
      versions.sort((a, b) => compare(a.version, b.version));
      const newest = versions.at(-1)!.listing;
      if (versions.some((v) => v.listing.kind !== newest.kind)) problems.push(`listings/${id}: a listing keeps its kind across versions`);
      const latest = latestOf(versions);
      entries.push({
        id,
        kind: newest.kind,
        title: newest.title,
        summary: newest.summary,
        tier: (FIRST_PARTY_NAMESPACES as readonly string[]).includes(namespace) ? 'first-party' : 'community',
        ...(latest !== undefined ? { latest } : {}),
        versions,
      });
    }
  }

  let revocations: unknown[] = [];
  const revocationsFile = join(root, 'revocations.json');
  if (existsSync(revocationsFile)) {
    const doc = JSON.parse(readFileSync(revocationsFile, 'utf8')) as { revocations?: unknown[] };
    for (const issue of validateAgainst('revocations.v1.json', doc)) problems.push(`revocations.json${issue.path ? `#${issue.path}` : ''}: ${issue.message}`);
    revocations = (doc.revocations ?? []) as unknown[];
    revocations.forEach((entry, i) => {
      const range = (entry as { versions?: unknown }).versions;
      if (typeof range === 'string' && !isRange(range)) problems.push(`revocations.json#/revocations/${i}/versions: ${range} is not a version range`);
    });
    revocations = [...revocations].sort((a, b) => canonical(a).localeCompare(canonical(b)));
  }

  if (problems.length > 0) throw new IndexError(problems);
  entries.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  const index: Index = {
    $schema: INDEX_SCHEMA_ID,
    index_version: 1,
    ...(options.sourceCommit ? { source_commit: options.sourceCommit } : {}),
    listings: entries,
  };
  const indexIssues = validateAgainst('index.v1.json', index);
  if (indexIssues.length > 0) throw new IndexError(indexIssues.map((i) => `index.json${i.path ? `#${i.path}` : ''}: ${i.message}`));
  return { index, revocations: { $schema: REVOCATIONS_SCHEMA_ID, revocations }, warnings };
}

/** Writes index.json and revocations.json into `out`; returns their paths. */
export function writeIndex(result: BuildResult, out: string): string[] {
  mkdirSync(out, { recursive: true });
  const files = [join(out, 'index.json'), join(out, 'revocations.json')];
  writeFileSync(files[0]!, canonical(result.index));
  writeFileSync(files[1]!, canonical(result.revocations));
  return files;
}
