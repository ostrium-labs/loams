// `loams-listing verify`: the index CI's artifact resolution, integrity and
// provenance check (design 73 section 6, D1459; plan MK1 Task 3, Task 0
// rulings 5 and Task 1 ruling 8). It downloads what a listing points at,
// hashes the bytes, verifies the Sigstore bundle of the SLSA provenance, and
// requires the provenance to name the listing's source repository, both in
// the signed statement and in the signing certificate. agentd repeats the
// same checks in Rust before an install (Task 6).
import { createHash } from 'node:crypto';
import { integrityOf, normaliseRepository } from './from-package.js';
import type { ArchiveFile, ArtifactLocation, Listing } from './types.js';
import { epochStatus, estimateEpoch, readVerified, type WalrusRecord } from './walrus.js';

export const SLSA_V1 = 'https://slsa.dev/provenance/v1';

/** OIDC issuers whose workflow identities the marketplace accepts. */
export const TRUSTED_ISSUERS = ['https://token.actions.githubusercontent.com', 'https://gitlab.com'];

export interface Fetcher {
  json(url: string): Promise<unknown>;
  bytes(url: string): Promise<Uint8Array>;
}

export interface SignerIdentity {
  subjectAlternativeName?: string;
  issuer?: string;
}

/** Verifies a Sigstore bundle cryptographically (signature, certificate chain, transparency log). Throws on failure. */
export interface BundleVerifier {
  verify(bundle: unknown): Promise<SignerIdentity>;
}

export interface Check {
  check: string;
  ok: boolean;
  detail: string;
  /** A passing check worth a look (an unread mirror, a blob near its end epoch). */
  warning?: true;
}

export interface VerifyReport {
  id: string;
  version: string;
  ok: boolean;
  checks: Check[];
  /** The provenance bundles that passed, to store beside the listing (versions/<v>/provenance/). */
  bundles: { name: string; bundle: unknown }[];
}

interface Statement {
  subject?: { name?: string; digest?: Record<string, string> }[];
  predicateType?: string;
  predicate?: { buildDefinition?: { externalParameters?: { workflow?: { repository?: string; path?: string; ref?: string } } } };
}

interface BundleJson {
  dsseEnvelope?: { payload?: string; payloadType?: string };
}

function statementOf(bundle: unknown): Statement | undefined {
  const payload = (bundle as BundleJson | undefined)?.dsseEnvelope?.payload;
  if (typeof payload !== 'string') return undefined;
  try {
    return JSON.parse(Buffer.from(payload, 'base64').toString('utf8')) as Statement;
  } catch {
    return undefined;
  }
}

function sameRepository(a: string, b: string): boolean {
  return normaliseRepository(a).toLowerCase() === normaliseRepository(b).toLowerCase();
}

function sha256Hex(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex');
}

/** `sha512-<base64>` → hex, as in-toto subjects carry it. */
export function integrityHex(integrity: string): string | undefined {
  const m = /^sha512-(.+)$/.exec(integrity);
  return m ? Buffer.from(m[1]!, 'base64').toString('hex') : undefined;
}

export function npmDocumentUrl(name: string, version: string): string {
  return `https://registry.npmjs.org/${name.replaceAll('/', '%2f')}/${encodeURIComponent(version)}`;
}

export interface VerifyOptions {
  fetcher: Fetcher;
  verifier: BundleVerifier;
  /** The walrus checks (plan MS1 Task 9); absent, walrus locations are only warned about. */
  walrus?: WalrusVerifyOptions;
}

export interface WalrusVerifyOptions {
  /** Read every walrus location here and compare its SHA-256. */
  aggregator?: URL;
  /** The walrus record beside the listing: where end epochs come from (ruling 2). */
  record?: WalrusRecord;
  /** The network's epoch; estimated from the record when absent. */
  currentEpoch?: number;
  /** Q1332: warn when a blob ends within this many epochs. */
  supportEpochs: number;
  maxBytes: number;
  now?: () => Date;
}

class Report {
  readonly checks: Check[] = [];
  readonly bundles: { name: string; bundle: unknown }[] = [];
  pass(check: string, detail: string): void {
    this.checks.push({ check, ok: true, detail });
  }
  fail(check: string, detail: string): false {
    this.checks.push({ check, ok: false, detail });
    return false;
  }
  warn(check: string, detail: string): void {
    this.checks.push({ check, ok: true, detail, warning: true });
  }
}

function walrusLocations(listing: Listing): { label: string; location: ArtifactLocation }[] {
  const artifact = listing.artifact;
  if (!artifact) return [];
  const out = (artifact.locations ?? []).filter((l) => l.kind === 'walrus').map((location) => ({ label: 'package', location }));
  for (const file of artifact.files ?? []) {
    for (const location of file.locations ?? []) if (location.kind === 'walrus') out.push({ label: file.platform, location });
  }
  return out;
}

/**
 * Each walrus location: read and SHA-256-checked through the aggregator, and its end
 * epoch (from the walrus record) against the support window (Q1332).
 */
async function verifyWalrus(listing: Listing, report: Report, options: WalrusVerifyOptions | undefined): Promise<void> {
  const locations = walrusLocations(listing);
  if (locations.length === 0) return;
  const now = options?.now?.() ?? new Date();
  const record = options?.record;
  const current = options?.currentEpoch ?? (record ? estimateEpoch(record, now) : undefined);
  for (const { label, location } of locations) {
    const id = `${label}: ${location.blob_id}`;
    if (options?.aggregator) {
      try {
        await readVerified(location.blob_id, location.sha256, { aggregator: options.aggregator, retry404Ms: 0, maxBytes: options.maxBytes });
        report.pass('walrus_read', `${id} matches sha256 ${location.sha256.slice(0, 12)}…`);
      } catch (error) {
        report.fail('walrus_read', `${id}: ${(error as Error).message}`);
      }
    } else {
      report.warn('walrus_read', `${id} not read: no --aggregator`);
    }
    const recorded = record?.blobs.find((b) => b.blob_id === location.blob_id);
    if (!recorded) {
      report.warn('walrus_epoch', `${id}: no end epoch in the walrus record, so its expiry is not checked`);
      continue;
    }
    if (current === undefined) {
      report.warn('walrus_epoch', `${id}: stored until epoch ${recorded.end_epoch}; the current epoch is unknown (give --current-epoch)`);
      continue;
    }
    const status = epochStatus(recorded.end_epoch, current, options!.supportEpochs);
    const left = status.endEpoch - status.current;
    if (status.state === 'expired') {
      report.fail('walrus_epoch', `${id}: storage ended at epoch ${status.endEpoch}, and the current epoch is ${status.current}; store it again (installs fall back to HTTPS)`);
    } else if (status.state === 'expiring') {
      report.warn('walrus_epoch', `${id}: storage ends at epoch ${status.endEpoch}, ${left} epoch(s) from the current ${status.current}, inside the ${options!.supportEpochs}-epoch support window; store it again`);
    } else {
      report.pass('walrus_epoch', `${id}: stored until epoch ${status.endEpoch} (current ${status.current})`);
    }
  }
}

/**
 * Checks one SLSA provenance bundle: the signature and certificate (through the verifier),
 * the certificate's issuer and workflow identity, the statement's subject digest, and the
 * statement's source repository.
 */
async function checkProvenance(
  report: Report,
  label: string,
  bundles: unknown[],
  subject: { algorithm: 'sha512' | 'sha256'; hex: string },
  listing: Listing,
  verifier: BundleVerifier,
): Promise<boolean> {
  const slsa = bundles.filter((b) => statementOf(b)?.predicateType === SLSA_V1);
  if (slsa.length === 0) return report.fail('provenance_present', `${label}: no SLSA v1 provenance`);
  const repository = listing.source.repository;
  const reasons: string[] = [];
  for (const bundle of slsa) {
    const statement = statementOf(bundle)!;
    if (!(statement.subject ?? []).some((s) => s.digest?.[subject.algorithm]?.toLowerCase() === subject.hex)) {
      reasons.push('the statement does not cover these bytes');
      continue;
    }
    const workflow = statement.predicate?.buildDefinition?.externalParameters?.workflow;
    if (!workflow?.repository || !sameRepository(workflow.repository, repository)) {
      reasons.push(`built from ${workflow?.repository ?? 'an unnamed repository'}, not ${repository}`);
      continue;
    }
    let identity: SignerIdentity;
    try {
      identity = await verifier.verify(bundle);
    } catch (error) {
      reasons.push(`the bundle does not verify: ${(error as Error).message}`);
      continue;
    }
    if (!identity.issuer || !TRUSTED_ISSUERS.includes(identity.issuer)) {
      reasons.push(`signed by an untrusted issuer ${identity.issuer ?? '(none)'}`);
      continue;
    }
    const san = identity.subjectAlternativeName ?? '';
    if (!san.toLowerCase().startsWith(`${normaliseRepository(repository).toLowerCase()}/`)) {
      reasons.push(`signed by ${san || 'an unnamed identity'}, not a workflow of ${repository}`);
      continue;
    }
    report.bundles.push({ name: label, bundle });
    report.pass('provenance', `${label}: built by ${san}`);
    return true;
  }
  return report.fail('provenance', `${label}: ${reasons.join('; ')}`);
}

async function verifyNpm(listing: Listing, report: Report, { fetcher, verifier }: VerifyOptions): Promise<void> {
  const artifact = listing.artifact!;
  const doc = (await fetcher.json(npmDocumentUrl(artifact.name, artifact.version))) as {
    dist?: { integrity?: string; tarball?: string; attestations?: { url?: string } };
  };
  const dist = doc.dist ?? {};
  if (dist.integrity !== artifact.integrity) {
    report.fail('integrity', `the registry's integrity ${dist.integrity ?? '(none)'} is not the listing's ${artifact.integrity}`);
    return;
  }
  if (!dist.tarball) {
    report.fail('artifact_resolves', 'the registry names no tarball');
    return;
  }
  const bytes = await fetcher.bytes(dist.tarball);
  const actual = integrityOf(bytes);
  if (actual !== artifact.integrity) {
    report.fail('integrity', `the tarball hashes to ${actual}, not ${artifact.integrity}`);
    return;
  }
  report.pass('integrity', `${artifact.name}@${artifact.version} matches ${artifact.integrity.slice(0, 19)}…`);
  if (listing.provenance?.type !== 'npm-slsa') {
    report.fail('provenance', `npm artifacts need npm-slsa provenance, not ${listing.provenance?.type ?? 'none'}`);
    return;
  }
  const url = dist.attestations?.url ?? listing.provenance.ref;
  const attestations = (await fetcher.json(url)) as { attestations?: { bundle?: unknown }[] };
  const bundles = (attestations.attestations ?? []).map((a) => a.bundle);
  await checkProvenance(report, `${artifact.name}@${artifact.version}`, bundles, { algorithm: 'sha512', hex: integrityHex(artifact.integrity!)! }, listing, verifier);
}

async function verifyArchiveFile(
  listing: Listing,
  file: ArchiveFile,
  report: Report,
  { fetcher, verifier }: VerifyOptions,
): Promise<void> {
  const bytes = await fetcher.bytes(file.url);
  const actual = sha256Hex(bytes);
  if (actual !== file.sha256) {
    report.fail('integrity', `${file.platform}: ${file.url} hashes to ${actual}, not ${file.sha256}`);
    return;
  }
  report.pass('integrity', `${file.platform}: sha256 ${file.sha256.slice(0, 12)}…`);
  const provenance = listing.provenance!;
  let bundles: unknown[];
  if (provenance.type === 'github-attestation') {
    const found = (await fetcher.json(`${provenance.ref.replace(/\/$/, '')}/sha256:${file.sha256}`)) as { attestations?: { bundle?: unknown }[] };
    bundles = (found.attestations ?? []).map((a) => a.bundle);
  } else if (provenance.type === 'sigstore-bundle') {
    bundles = [await fetcher.json(provenance.ref)];
  } else {
    report.fail('provenance', `archives need github-attestation or sigstore-bundle provenance, not ${provenance.type}`);
    return;
  }
  await checkProvenance(report, `${listing.artifact!.name} ${file.platform}`, bundles, { algorithm: 'sha256', hex: file.sha256 }, listing, verifier);
}

export async function verifyListing(listing: Listing, options: VerifyOptions): Promise<VerifyReport> {
  const report = new Report();
  const artifact = listing.artifact;
  try {
    if (!artifact) {
      report.pass('artifact', listing.kind === 'mcp-server' ? 'a remote MCP server installs nothing' : 'a built-in harness installs nothing');
    } else if (artifact.type === 'npm') {
      await verifyNpm(listing, report, options);
    } else if (artifact.type === 'archive') {
      for (const file of artifact.files ?? []) await verifyArchiveFile(listing, file, report, options);
    } else {
      report.fail('artifact', `${artifact.type} artifacts are verified from MK1 (pypi) and MK2 (oci)`);
    }
  } catch (error) {
    report.fail('artifact_resolves', (error as Error).message);
  }
  await verifyWalrus(listing, report, options.walrus);
  return {
    id: listing.id,
    version: listing.version,
    ok: report.checks.length > 0 && report.checks.every((c) => c.ok),
    checks: report.checks,
    bundles: report.bundles,
  };
}

/** The network fetcher (Node's fetch), with a size cap so a listing cannot make CI download without end. */
export function httpFetcher(maxBytes = 512 * 1024 * 1024): Fetcher {
  async function get(url: string, accept: string): Promise<Response> {
    if (!url.startsWith('https://')) throw new Error(`refusing a non-https URL: ${url}`);
    const response = await fetch(url, { headers: { accept, 'user-agent': 'loams-listing' }, redirect: 'follow' });
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return response;
  }
  return {
    async json(url) {
      return (await get(url, 'application/json')).json();
    },
    async bytes(url) {
      const response = await get(url, 'application/octet-stream');
      const length = Number(response.headers.get('content-length') ?? '0');
      if (length > maxBytes) throw new Error(`${url}: ${length} bytes is over the ${maxBytes}-byte cap`);
      const bytes = new Uint8Array(await response.arrayBuffer());
      if (bytes.length > maxBytes) throw new Error(`${url}: over the ${maxBytes}-byte cap`);
      return bytes;
    },
  };
}

/** Sigstore's public-good verification (sigstore-js; the trusted root through TUF). */
export async function sigstoreVerifier(): Promise<BundleVerifier> {
  const { verify } = await import('sigstore');
  return {
    async verify(bundle) {
      const signer = await verify(bundle as Parameters<typeof verify>[0]);
      return { subjectAlternativeName: signer.identity?.subjectAlternativeName, issuer: signer.identity?.extensions?.issuer };
    },
  };
}
