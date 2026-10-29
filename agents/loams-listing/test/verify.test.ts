import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vite-plus/test';
import { integrityOf } from '../src/from-package.js';
import type { Listing } from '../src/types.js';
import { type BundleVerifier, type Fetcher, integrityHex, npmDocumentUrl, SLSA_V1, verifyListing } from '../src/verify.js';
import { fixture } from './helpers.js';

const GITHUB = 'https://token.actions.githubusercontent.com';
const TARBALL = new TextEncoder().encode('the starter agent, packed');
const INTEGRITY = integrityOf(TARBALL);

interface FakeBundle {
  dsseEnvelope: { payloadType: string; payload: string };
  /** What the fake verifier reports as the signer; `invalid` makes it throw. */
  signer: { san?: string; issuer?: string; invalid?: boolean };
}

function bundle(statement: object, signer: FakeBundle['signer']): FakeBundle {
  return {
    dsseEnvelope: { payloadType: 'application/vnd.in-toto+json', payload: Buffer.from(JSON.stringify(statement)).toString('base64') },
    signer,
  };
}

function slsa(subject: object[], repository: string): object {
  return {
    _type: 'https://in-toto.io/Statement/v1',
    subject,
    predicateType: SLSA_V1,
    predicate: { buildDefinition: { externalParameters: { workflow: { repository, path: '.github/workflows/release.yml', ref: 'refs/heads/main' } } } },
  };
}

const verifier: BundleVerifier = {
  async verify(b) {
    const { signer } = b as FakeBundle;
    if (signer.invalid) throw new Error('signature does not match');
    return { subjectAlternativeName: signer.san, issuer: signer.issuer };
  },
};

function fetcher(json: Record<string, unknown>, bytes: Record<string, Uint8Array>): Fetcher & { fetched: string[] } {
  const fetched: string[] = [];
  return {
    fetched,
    async json(url) {
      fetched.push(url);
      if (!(url in json)) throw new Error(`${url}: HTTP 404`);
      return json[url];
    },
    async bytes(url) {
      fetched.push(url);
      if (!(url in bytes)) throw new Error(`${url}: HTTP 404`);
      return bytes[url]!;
    },
  };
}

function npmListing(): Listing {
  const listing = fixture('valid/agent-mastra-acp.json');
  listing.artifact!.integrity = INTEGRITY;
  return listing;
}

const REPO = 'https://github.com/ostrium-labs/loams-mastra';
const SAN = `${REPO}/.github/workflows/release.yml@refs/heads/main`;

function npmWorld(overrides: { integrity?: string; tarball?: Uint8Array; bundles?: FakeBundle[] } = {}) {
  const listing = npmListing();
  const { name, version } = listing.artifact!;
  const tarballUrl = `https://registry.npmjs.org/${name}/-/starter-agent-${version}.tgz`;
  const attestations = `https://registry.npmjs.org/-/npm/v1/attestations/${name}@${version}`;
  const subject = [{ name: `pkg:npm/${name}@${version}`, digest: { sha512: integrityHex(INTEGRITY) } }];
  const bundles = overrides.bundles ?? [bundle(slsa(subject, REPO), { san: SAN, issuer: GITHUB })];
  return {
    listing,
    subject,
    fetcher: fetcher(
      {
        [npmDocumentUrl(name, version)]: { dist: { integrity: overrides.integrity ?? INTEGRITY, tarball: tarballUrl, attestations: { url: attestations } } },
        [attestations]: { attestations: bundles.map((b) => ({ predicateType: SLSA_V1, bundle: b })) },
      },
      { [tarballUrl]: overrides.tarball ?? TARBALL },
    ),
  };
}

describe('verify: npm', () => {
  it('accepts matching bytes and provenance from the source repository', async () => {
    const world = npmWorld();
    const report = await verifyListing(world.listing, { fetcher: world.fetcher, verifier });
    expect(report.checks.filter((c) => !c.ok)).toEqual([]);
    expect(report.ok).toBe(true);
    expect(report.bundles).toHaveLength(1);
    expect(world.fetcher.fetched[0]).toBe('https://registry.npmjs.org/@loams%2fstarter-agent/0.1.0');
  });

  it('integrity_mismatch_refused: the registry names other bytes', async () => {
    const world = npmWorld({ integrity: integrityOf(new TextEncoder().encode('other')) });
    const report = await verifyListing(world.listing, { fetcher: world.fetcher, verifier });
    expect(report.ok).toBe(false);
    expect(report.checks).toContainEqual(expect.objectContaining({ check: 'integrity', ok: false }));
  });

  it('integrity_mismatch_refused: the tarball is not what the registry says', async () => {
    const world = npmWorld({ tarball: new TextEncoder().encode('swapped on the wire') });
    const report = await verifyListing(world.listing, { fetcher: world.fetcher, verifier });
    expect(report.ok).toBe(false);
    expect(report.checks.find((c) => c.check === 'integrity')?.detail).toMatch(/hashes to/);
  });

  it('provenance_repo_mismatch_refused: built from another repository', async () => {
    const subject = [{ name: 'x', digest: { sha512: integrityHex(INTEGRITY) } }];
    const world = npmWorld({ bundles: [bundle(slsa(subject, 'https://github.com/attacker/fork'), { san: 'https://github.com/attacker/fork/.github/workflows/release.yml@refs/heads/main', issuer: GITHUB })] });
    const report = await verifyListing(world.listing, { fetcher: world.fetcher, verifier });
    expect(report.ok).toBe(false);
    expect(report.checks.find((c) => c.check === 'provenance')?.detail).toMatch(/attacker\/fork/);
  });

  it('provenance_repo_mismatch_refused: the statement names the source but another workflow signed it', async () => {
    const subject = [{ name: 'x', digest: { sha512: integrityHex(INTEGRITY) } }];
    const world = npmWorld({ bundles: [bundle(slsa(subject, REPO), { san: 'https://github.com/attacker/fork/.github/workflows/release.yml@refs/heads/main', issuer: GITHUB })] });
    expect((await verifyListing(world.listing, { fetcher: world.fetcher, verifier })).ok).toBe(false);
  });

  it('refuses a bundle that does not verify, an untrusted issuer, or other bytes', async () => {
    for (const b of [
      bundle(slsa([{ digest: { sha512: integrityHex(INTEGRITY) } }], REPO), { san: SAN, issuer: GITHUB, invalid: true }),
      bundle(slsa([{ digest: { sha512: integrityHex(INTEGRITY) } }], REPO), { san: SAN, issuer: 'https://accounts.example.com' }),
      bundle(slsa([{ digest: { sha512: 'ab'.repeat(64) } }], REPO), { san: SAN, issuer: GITHUB }),
    ]) {
      const world = npmWorld({ bundles: [b] });
      expect((await verifyListing(world.listing, { fetcher: world.fetcher, verifier })).ok).toBe(false);
    }
  });

  it('refuses a package with no provenance', async () => {
    const world = npmWorld({ bundles: [] });
    const report = await verifyListing(world.listing, { fetcher: world.fetcher, verifier });
    expect(report.checks).toContainEqual(expect.objectContaining({ check: 'provenance_present', ok: false }));
  });

  it('reports an unreachable registry as a failure, not a crash', async () => {
    const report = await verifyListing(npmListing(), { fetcher: fetcher({}, {}), verifier });
    expect(report.ok).toBe(false);
    expect(report.checks[0]?.check).toBe('artifact_resolves');
  });
});

describe('verify: archives with GitHub attestations', () => {
  function archiveWorld(tamper?: string) {
    const listing = fixture('valid/mcp-server-local.json');
    const json: Record<string, unknown> = {};
    const bytes: Record<string, Uint8Array> = {};
    for (const file of listing.artifact!.files!) {
      const data = new TextEncoder().encode(`archive for ${file.platform}`);
      file.sha256 = createHash('sha256').update(data).digest('hex');
      bytes[file.url] = file.platform === tamper ? new TextEncoder().encode('tampered') : data;
      json[`https://api.github.com/repos/github/github-mcp-server/attestations/sha256:${file.sha256}`] = {
        attestations: [
          {
            bundle: bundle(slsa([{ name: file.url.split('/').pop(), digest: { sha256: file.sha256 } }], 'https://github.com/github/github-mcp-server'), {
              san: 'https://github.com/github/github-mcp-server/.github/workflows/goreleaser.yml@refs/tags/v2.0.3',
              issuer: GITHUB,
            }),
          },
        ],
      };
    }
    return { listing, fetcher: fetcher(json, bytes) };
  }

  it('checks every platform file and its attestation', async () => {
    const world = archiveWorld();
    const report = await verifyListing(world.listing, { fetcher: world.fetcher, verifier });
    expect(report.checks.filter((c) => !c.ok)).toEqual([]);
    expect(report.checks.filter((c) => c.check === 'provenance')).toHaveLength(3);
  });

  it('integrity_mismatch_refused: one platform file differs', async () => {
    const world = archiveWorld('windows-x86_64');
    const report = await verifyListing(world.listing, { fetcher: world.fetcher, verifier });
    expect(report.ok).toBe(false);
    expect(report.checks.find((c) => !c.ok)?.detail).toMatch(/windows-x86_64/);
  });
});

describe('verify: listings without an artifact', () => {
  it('passes a remote MCP server and a native harness', async () => {
    for (const name of ['valid/mcp-server-remote.json', 'valid/harness-native.json']) {
      const report = await verifyListing(fixture(name), { fetcher: fetcher({}, {}), verifier });
      expect(report.ok, name).toBe(true);
    }
  });

  it('refuses the runtimes MK0 cannot verify yet', async () => {
    const report = await verifyListing(fixture('valid/agent-pydantic-ai-a2a.json'), { fetcher: fetcher({}, {}), verifier });
    expect(report.ok).toBe(false);
  });
});
