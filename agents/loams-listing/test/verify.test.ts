import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vite-plus/test';
import { integrityOf } from '../src/from-package.js';
import type { Listing } from '../src/types.js';
import type { OciHttp } from '../src/oci.js';
import { type BundleVerifier, type Fetcher, integrityHex, npmDocumentUrl, SLSA_V1, verifyListing, verifyPackage, type PackageDeps, type PackageListing } from '../src/verify.js';
import { fixture, indexBody, INDEX_TYPE } from './helpers.js';

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

describe('verifyPackage (listing/v2)', () => {
  const REPO_URL = 'https://github.com/acme/notes';
  const SAN_V2 = `${REPO_URL}/.github/workflows/release.yml@refs/heads/main`;
  const hex = (b: Uint8Array) => createHash('sha256').update(b).digest('hex');
  const both = indexBody(['linux/amd64', 'linux/arm64']);
  const amdOnly = indexBody(['linux/amd64']);

  /** A registry holding the given index bodies by digest. */
  function registry(manifests: Record<string, { body: string }>, missing = false): OciHttp {
    return async (r) => {
      const digest = r.url.split('/manifests/')[1]!;
      const m = manifests[digest];
      if (!m || missing) return { status: 404, headers: {} as Record<string, string>, body: new Uint8Array() };
      return { status: 200, headers: { 'content-type': INDEX_TYPE } as Record<string, string>, body: r.method === 'GET' ? new TextEncoder().encode(m.body) : new Uint8Array() };
    };
  }

  function deps(over: Partial<PackageDeps> & { json?: Record<string, unknown>; bytes?: Record<string, Uint8Array> } = {}): PackageDeps & { cosign: string[][] } {
    const cosign: string[][] = [];
    return {
      cosign,
      fetcher: over.fetcher ?? fetcher(over.json ?? {}, over.bytes ?? {}),
      oci: over.oci ?? registry({ [both.digest]: both }),
      verifier,
      runCosign: over.runCosign ?? (async (args) => (cosign.push(args), { code: 0, stdout: '[]', stderr: '' })),
    };
  }

  const oci = (digest = both.digest, kind = 'app', provenance: PackageListing['provenance'] = { type: 'cosign', ref: 'unused' }): PackageListing => ({
    id: 'acme/notes',
    kind,
    source: { repository: REPO_URL },
    package: { type: 'oci', image: 'ghcr.io/acme/notes', digest },
    provenance,
  });

  function attestation(hexDigest: string, san = SAN_V2): { url: string; doc: unknown } {
    const subject = [{ name: 'x', digest: { sha256: hexDigest } }];
    return {
      url: `https://api.github.com/repos/acme/notes/attestations/sha256:${hexDigest}`,
      doc: { attestations: [{ bundle: bundle(slsa(subject, REPO_URL), { san, issuer: GITHUB }) }] },
    };
  }

  it('oci_digest_resolves', async () => {
    const d = deps();
    const r = await verifyPackage(oci(), d);
    expect(r).toMatchObject({ digest: 'ok', provenance: 'ok' });
  });

  it('oci_digest_missing_fails', async () => {
    const r = await verifyPackage(oci(), deps({ oci: registry({}) }));
    expect(r.digest).toBe('fail');
    expect(r.detail).toMatch(/no such manifest/);
  });

  it('oci_index_must_include_linux_amd64_and_arm64', async () => {
    const r = await verifyPackage(oci(amdOnly.digest), deps({ oci: registry({ [amdOnly.digest]: amdOnly }) }));
    expect(r.digest).toBe('fail');
    expect(r.detail).toMatch(/linux\/arm64/);
    // an agent-kind image is not held to D2626
    const agent = await verifyPackage(oci(amdOnly.digest, 'agent'), deps({ oci: registry({ [amdOnly.digest]: amdOnly }) }));
    expect(agent.digest).toBe('ok');
  });

  it('helm_oci_chart_digest_resolves', async () => {
    const d = deps();
    const listing: PackageListing = { ...oci(), package: { type: 'helm', chart: 'oci://ghcr.io/acme/charts/notes', version: '1.0.0', digest: both.digest } };
    expect(await verifyPackage(listing, d)).toMatchObject({ digest: 'ok', provenance: 'ok' });
    expect(d.cosign[0]![1]).toBe(`ghcr.io/acme/charts/notes@${both.digest}`);
    const gone = await verifyPackage(listing, deps({ oci: registry({}) }));
    expect(gone.digest).toBe('fail');
  });

  const FILE = new TextEncoder().encode('chart bytes');
  const att = (b: Uint8Array) => attestation(hex(b));

  it('helm_https_tgz_sha256_checked', async () => {
    const url = 'https://example.com/wiki-3.1.0.tgz';
    const a = att(FILE);
    const listing: PackageListing = { ...oci(), package: { type: 'helm', chart: url, version: '3.1.0', digest: `sha256:${hex(FILE)}` }, provenance: { type: 'github-attestation', ref: 'https://api.github.com/repos/acme/notes/attestations' } };
    expect(await verifyPackage(listing, deps({ bytes: { [url]: FILE }, json: { [a.url]: a.doc } }))).toMatchObject({ digest: 'ok', provenance: 'ok' });
    const bad = await verifyPackage(listing, deps({ bytes: { [url]: new TextEncoder().encode('other') } }));
    expect(bad.digest).toBe('fail');
    expect(bad.detail).toMatch(/hashes to/);
  });

  it('kustomize_tarball_sha256_checked', async () => {
    const url = 'https://example.com/kustomize.tar.gz';
    const a = att(FILE);
    const listing: PackageListing = { ...oci(), kind: 'engine', package: { type: 'kustomize', url, digest: `sha256:${hex(FILE)}` }, provenance: { type: 'github-attestation', ref: 'https://api.github.com/repos/acme/notes/attestations' } };
    expect(await verifyPackage(listing, deps({ bytes: { [url]: FILE }, json: { [a.url]: a.doc } }))).toMatchObject({ digest: 'ok', provenance: 'ok' });
    expect((await verifyPackage(listing, deps({ bytes: { [url]: new Uint8Array([1]) } }))).digest).toBe('fail');
  });

  it('binary_each_triple_checked', async () => {
    const one = new TextEncoder().encode('linux'), two = new TextEncoder().encode('mac');
    const urls = { a: 'https://example.com/syncd-linux', b: 'https://example.com/syncd-mac' };
    const listing: PackageListing = {
      ...oci(),
      package: { type: 'binary', targets: [{ triple: 'x86_64-unknown-linux-gnu', url: urls.a, digest: `sha256:${hex(one)}` }, { triple: 'aarch64-apple-darwin', url: urls.b, digest: `sha256:${hex(two)}` }] },
      provenance: { type: 'github-attestation', ref: 'https://api.github.com/repos/acme/notes/attestations' },
    };
    const a1 = att(one), a2 = att(two);
    const json = { [a1.url]: a1.doc, [a2.url]: a2.doc };
    expect(await verifyPackage(listing, deps({ bytes: { [urls.a]: one, [urls.b]: two }, json }))).toMatchObject({ digest: 'ok', provenance: 'ok' });
    const bad = await verifyPackage(listing, deps({ bytes: { [urls.a]: one, [urls.b]: new Uint8Array([9]) }, json }));
    expect(bad.digest).toBe('fail');
    expect(bad.detail).toMatch(/aarch64-apple-darwin/);
  });

  it('cosign_signer_must_be_source_repo_workflow', async () => {
    const d = deps();
    await verifyPackage(oci(), d);
    expect(d.cosign).toEqual([
      ['verify', `ghcr.io/acme/notes@${both.digest}`, '--certificate-identity-regexp', '^https://github\\.com/acme/notes/\\.github/workflows/', '--certificate-oidc-issuer', GITHUB, '--output', 'json'],
    ]);
    const failing = deps({ runCosign: async () => ({ code: 1, stdout: '', stderr: 'no matching signatures\n' }) });
    expect(await verifyPackage(oci(), failing)).toMatchObject({ digest: 'ok', provenance: 'fail' });
  });

  it('cosign is refused for a file package', async () => {
    const url = 'https://example.com/f.wasm';
    const listing: PackageListing = { ...oci(), package: { type: 'wasm', url, digest: `sha256:${hex(FILE)}` } };
    const r = await verifyPackage(listing, deps({ bytes: { [url]: FILE } }));
    expect(r.provenance).toBe('fail');
    expect(r.detail).toMatch(/OCI/);
  });

  it('github_attestation_by_package_digest', async () => {
    const a = attestation(both.digest.slice(7));
    const listing = oci(both.digest, 'app', { type: 'github-attestation', ref: 'https://api.github.com/repos/acme/notes/attestations' });
    const d = deps({ json: { [a.url]: a.doc } });
    expect(await verifyPackage(listing, d)).toMatchObject({ digest: 'ok', provenance: 'ok' });
    // signed by another repository's workflow
    const wrong = attestation(both.digest.slice(7), 'https://github.com/evil/notes/.github/workflows/r.yml@refs/heads/main');
    expect((await verifyPackage(listing, deps({ json: { [wrong.url]: wrong.doc } }))).provenance).toBe('fail');
  });

  it('sigstore_bundle covers a single file only', async () => {
    const url = 'https://example.com/f.wasm';
    const ref = 'https://example.com/f.wasm.sigstore.json';
    const a = att(FILE);
    const listing: PackageListing = { ...oci(), package: { type: 'wasm', url, digest: `sha256:${hex(FILE)}` }, provenance: { type: 'sigstore-bundle', ref } };
    const bundleDoc = (a.doc as { attestations: { bundle: unknown }[] }).attestations[0]!.bundle;
    expect(await verifyPackage(listing, deps({ bytes: { [url]: FILE }, json: { [ref]: bundleDoc } }))).toMatchObject({ provenance: 'ok' });
    const image = await verifyPackage({ ...oci(), provenance: { type: 'sigstore-bundle', ref } }, deps({ json: { [ref]: bundleDoc } }));
    expect(image.provenance).toBe('fail');
    expect(image.detail).toMatch(/single file/);
  });

  it('provenance_missing_reported', async () => {
    const listing = { ...oci(), provenance: undefined };
    expect(await verifyPackage(listing, deps())).toMatchObject({ digest: 'ok', provenance: 'missing' });
  });

  it('verifyListing reports a v2 package as checks and no longer says oci is unverified', async () => {
    const listing = { ...oci(), $schema: 'x' } as unknown as Listing;
    const d = deps();
    const report = await verifyListing(listing, { fetcher: d.fetcher, verifier, oci: d.oci, runCosign: d.runCosign });
    expect(report.ok).toBe(true);
    expect(report.checks.map((c) => c.check)).toEqual(['package_digest', 'package_provenance']);
    const missing = await verifyListing({ ...oci(), provenance: undefined } as unknown as Listing, { fetcher: d.fetcher, verifier, oci: d.oci });
    expect(missing.ok).toBe(false);
  });
});
