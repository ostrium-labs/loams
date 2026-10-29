// `loams-listing publish --walrus` and the walrus checks of `verify`
// (plan MS1 Task 9), against a mock publisher and aggregator on loopback.
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { main } from '../src/cli.js';
import { integrityOf } from '../src/from-package.js';
import { hasErrors, lint } from '../src/lint.js';
import type { Listing } from '../src/types.js';
import type { VerifyOptions } from '../src/verify.js';
import { epochStatus, estimateEpoch, parseStoreAnswer, RECORD_SCHEMA, sha256Hex, type WalrusRecord } from '../src/walrus.js';
import { fixture } from './helpers.js';
import { blobIdOf, type MockWalrus, startMockWalrus } from './walrus-mock.js';

const TARBALL = new TextEncoder().encode('the starter agent, packed for walrus');
const LINUX = new TextEncoder().encode('fx for linux, archived');
const DARWIN = new TextEncoder().encode('fx for darwin, archived');
const NOW = new Date('2026-10-26T12:00:00Z');
const DAY = 24 * 60 * 60 * 1000;

let walrus: MockWalrus;
let dir: string;

beforeEach(async () => {
  walrus = await startMockWalrus();
  dir = mkdtempSync(join(tmpdir(), 'loams-listing-publish-'));
});

afterEach(async () => {
  await walrus.close();
  rmSync(dir, { recursive: true, force: true });
});

function npmListing(): Listing {
  const listing = fixture('valid/agent-mastra-acp.json');
  listing.artifact = { ...listing.artifact!, integrity: integrityOf(TARBALL) };
  delete listing.artifact.locations;
  return listing;
}

function archiveListing(): Listing {
  const listing = fixture('valid/harness-acp-binary.json');
  const [linux, darwin] = listing.artifact!.files!;
  linux!.sha256 = sha256Hex(LINUX);
  darwin!.sha256 = sha256Hex(DARWIN);
  delete linux!.locations;
  return listing;
}

function put(name: string, data: Uint8Array | string): string {
  writeFileSync(join(dir, name), data);
  return name;
}

function readJson<T>(name: string): T {
  return JSON.parse(readFileSync(join(dir, name), 'utf8')) as T;
}

/** No network beyond the mock, and no Sigstore: these listings' own artifact checks are not under test. */
const offline: VerifyOptions = {
  fetcher: {
    async json(url) {
      throw new Error(`${url}: offline`);
    },
    async bytes(url) {
      throw new Error(`${url}: offline`);
    },
  },
  verifier: {
    async verify() {
      throw new Error('offline');
    },
  },
};

async function run(args: string[], env: Record<string, string> = {}) {
  const out: string[] = [];
  const err: string[] = [];
  const code = await main(args, {
    stdout: (l) => out.push(l),
    stderr: (l) => err.push(l),
    cwd: dir,
    env,
    now: () => NOW,
    backoffMs: 5,
    verify: offline,
  });
  return { code, out, err };
}

function publishArgs(...extra: string[]): string[] {
  return ['publish', '--walrus', 'listing.json', '--publisher', walrus.url, '--aggregator', walrus.url, '--network', 'testnet', ...extra];
}

describe('publish --walrus', () => {
  it('publish_walrus_writes_location (npm tarball)', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    const result = await run(publishArgs('--tarball', 'pkg.tgz', '--epochs', '7'));
    expect(result.err).toEqual([]);
    expect(result.code).toBe(0);
    const id = blobIdOf(TARBALL);
    expect(result.out[0]).toBe(`stored package ${id} end_epoch 107 (newly created)`);
    expect(walrus.queries[0]!.get('epochs')).toBe('7');
    expect(walrus.queries[0]!.has('permanent')).toBe(false);
    const listing = readJson<Listing>('listing.json');
    expect(listing.artifact!.locations).toEqual([{ kind: 'walrus', blob_id: id, sha256: sha256Hex(TARBALL) }]);
    expect(hasErrors(lint(listing))).toBe(false);
    const record = readJson<WalrusRecord>('listing.walrus.json');
    expect(record).toEqual({
      schema: RECORD_SCHEMA,
      network: 'testnet',
      blobs: [{ blob_id: id, sha256: sha256Hex(TARBALL), end_epoch: 107, stored_at: NOW.toISOString(), epoch_at_store: 100 }],
    });
  });

  it('publish_walrus_writes_location (archive files)', async () => {
    put('listing.json', JSON.stringify(archiveListing()));
    put('linux.tgz', LINUX);
    put('darwin.tgz', DARWIN);
    const result = await run(publishArgs('--file', 'linux-x86_64=linux.tgz', '--file', 'darwin-aarch64=darwin.tgz', '--permanent', '--out', 'out.json'));
    expect(result.code).toBe(0);
    expect(walrus.puts).toBe(2);
    expect(walrus.queries[0]!.get('permanent')).toBe('true');
    const listing = readJson<Listing>('out.json');
    const [linux, darwin] = listing.artifact!.files!;
    expect(linux!.locations).toEqual([{ kind: 'walrus', blob_id: blobIdOf(LINUX), sha256: sha256Hex(LINUX) }]);
    expect(darwin!.locations).toEqual([{ kind: 'walrus', blob_id: blobIdOf(DARWIN), sha256: sha256Hex(DARWIN) }]);
    expect(hasErrors(lint(listing))).toBe(false);
    expect(readJson<WalrusRecord>('out.walrus.json').blobs).toHaveLength(2);
    expect(readJson<Listing>('listing.json').artifact!.files![0]!.locations).toBeUndefined();
  });

  it('publish_already_certified replaces the location and keeps one record entry', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    expect((await run(publishArgs('--tarball', 'pkg.tgz'))).code).toBe(0);
    const again = await run(publishArgs('--tarball', 'pkg.tgz'));
    expect(again.code).toBe(0);
    expect(again.out[0]).toMatch(/end_epoch 105 \(already certified\)$/);
    expect(readJson<Listing>('listing.json').artifact!.locations).toHaveLength(1);
    const [blob] = readJson<WalrusRecord>('listing.walrus.json').blobs;
    expect(blob!.end_epoch).toBe(105);
    // alreadyCertified does not say the current epoch.
    expect(blob!.epoch_at_store).toBeUndefined();
  });

  it('publish_readback_mismatch_refused: the listing is not changed', async () => {
    const original = JSON.stringify(npmListing());
    put('listing.json', original);
    put('pkg.tgz', TARBALL);
    walrus.tampered.add(blobIdOf(TARBALL));
    const result = await run(publishArgs('--tarball', 'pkg.tgz'));
    expect(result.code).toBe(1);
    expect(result.err.join('\n')).toMatch(/read-back .* failed, so the listing is not changed: .*not [0-9a-f]{64}/);
    expect(readFileSync(join(dir, 'listing.json'), 'utf8')).toBe(original);
    expect(existsSync(join(dir, 'listing.walrus.json'))).toBe(false);
  });

  it('publish_readback_retries_404', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    walrus.missFirst = 2;
    const result = await run(publishArgs('--tarball', 'pkg.tgz'));
    expect(result.code).toBe(0);
    expect(walrus.reads).toBe(3);
  });

  it('publish_readback_404_past_the_deadline_refused', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    walrus.missFirst = 1000;
    const result = await run(publishArgs('--tarball', 'pkg.tgz', '--read-after-write-ms', '30'));
    expect(result.code).toBe(1);
    expect(result.err.join('\n')).toMatch(/HTTP 404/);
  });

  it('publish_too_large_refused before any request', async () => {
    const big = new Uint8Array(1024).fill(7);
    const listing = npmListing();
    listing.artifact!.integrity = integrityOf(big);
    put('listing.json', JSON.stringify(listing));
    put('pkg.tgz', big);
    const result = await run(publishArgs('--tarball', 'pkg.tgz', '--max-bytes', '1000'));
    expect(result.code).toBe(1);
    expect(result.err.join('\n')).toMatch(/1024 bytes is over the 1000-byte limit/);
    expect(walrus.puts).toBe(0);
  });

  it('publish_bytes_not_the_listings_refused before any request', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('other.tgz', 'other bytes');
    const npm = await run(publishArgs('--tarball', 'other.tgz'));
    expect(npm.code).toBe(1);
    expect(npm.err.join('\n')).toMatch(/not the listing's sha512-/);
    put('listing.json', JSON.stringify(archiveListing()));
    const archive = await run(publishArgs('--file', 'linux-x86_64=other.tgz'));
    expect(archive.code).toBe(1);
    expect(archive.err.join('\n')).toMatch(/linux-x86_64: the file's sha256 is/);
    const unknown = await run(publishArgs('--file', 'windows-aarch64=other.tgz'));
    expect(unknown.err.join('\n')).toMatch(/no windows-aarch64 file/);
    const wrongKind = await run(publishArgs('--tarball', 'other.tgz'));
    expect(wrongKind.err.join('\n')).toMatch(/archive artifact publishes its files/);
    expect(walrus.puts).toBe(0);
  });

  it('publish_needs_publisher: there is no default', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    const result = await run(['publish', '--walrus', 'listing.json', '--tarball', 'pkg.tgz', '--aggregator', walrus.url, '--network', 'testnet']);
    expect(result.code).toBe(2);
    expect(result.err[0]).toMatch(/no default publisher/);
    const noNetwork = await run(['publish', '--walrus', 'listing.json', '--tarball', 'pkg.tgz', '--publisher', walrus.url, '--aggregator', walrus.url]);
    expect(noNetwork.code).toBe(2);
    expect((await run(['publish', 'listing.json'])).err[0]).toMatch(/needs --walrus/);
    expect((await run(publishArgs('--tarball', 'pkg.tgz', '--epochs', '54'))).code).toBe(2);
    expect(walrus.puts).toBe(0);
  });

  it('publish takes the publisher, aggregator and token file from the environment', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    put('token', 'sekrit\n');
    const result = await run(['publish', '--walrus', 'listing.json', '--tarball', 'pkg.tgz', '--network', 'mainnet'], {
      LOAMS_WALRUS_PUBLISHER: walrus.url,
      LOAMS_WALRUS_AGGREGATOR: walrus.url,
      LOAMS_WALRUS_PUBLISHER_TOKEN_FILE: 'token',
    });
    expect(result.code).toBe(0);
    expect(walrus.auth).toEqual(['Bearer sekrit']);
    expect(readJson<WalrusRecord>('listing.walrus.json').network).toBe('mainnet');
  });

  it('publish refuses a plain-http publisher off loopback', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    const result = await run(['publish', '--walrus', 'listing.json', '--tarball', 'pkg.tgz', '--network', 'testnet', '--publisher', 'http://publisher.example', '--aggregator', walrus.url]);
    expect(result.code).toBe(1);
    expect(result.err[0]).toMatch(/must be https/);
  });

  it('publish refuses a publisher that did not store the blob', async () => {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    walrus.answerWith = { status: 200, body: { markedInvalid: { blobId: 'x', event: {} } } };
    expect((await run(publishArgs('--tarball', 'pkg.tgz'))).err[0]).toMatch(/did not store the blob/);
    walrus.answerWith = { status: 500, body: { error: 'boom' } };
    expect((await run(publishArgs('--tarball', 'pkg.tgz'))).err[0]).toMatch(/HTTP 500/);
  });
});

describe('verify, walrus checks', () => {
  async function published(epochs = '5'): Promise<void> {
    put('listing.json', JSON.stringify(npmListing()));
    put('pkg.tgz', TARBALL);
    expect((await run(publishArgs('--tarball', 'pkg.tgz', '--epochs', epochs))).code).toBe(0);
  }

  function walrusLines(lines: string[]): string[] {
    return lines.filter((l) => l.includes('[walrus_'));
  }

  it('verify_reads_walrus_and_checks_sha256', async () => {
    await published();
    const ok = await run(['verify', 'listing.json', '--aggregator', walrus.url, '--current-epoch', '100']);
    expect(walrusLines(ok.out)).toEqual([
      expect.stringMatching(/ok \[walrus_read\] package: .* matches sha256/),
      expect.stringMatching(/ok \[walrus_epoch\] package: .* stored until epoch 105 \(current 100\)/),
    ]);
    walrus.tampered.add(blobIdOf(TARBALL));
    const bad = await run(['verify', 'listing.json', '--aggregator', walrus.url, '--current-epoch', '100']);
    expect(bad.code).toBe(1);
    expect(walrusLines(bad.err)).toEqual([expect.stringMatching(/FAILED \[walrus_read\] package: .*not [0-9a-f]{64}/)]);
  });

  it('verify warns when it cannot read the mirror', async () => {
    await published();
    const result = await run(['verify', 'listing.json', '--current-epoch', '100']);
    expect(walrusLines(result.err)).toEqual([expect.stringMatching(/WARNING \[walrus_read\] .* not read: no --aggregator/)]);
  });

  it('verify_warns_near_epoch_expiry', async () => {
    await published('5');
    const result = await run(['verify', 'listing.json', '--aggregator', walrus.url, '--current-epoch', '102']);
    expect(walrusLines(result.err)).toEqual([
      expect.stringMatching(/WARNING \[walrus_epoch\] package: .* ends at epoch 105, 3 epoch\(s\) from the current 102, inside the 4-epoch support window/),
    ]);
    // A warning only: the walrus checks pass (the listing fails on its offline artifact checks).
    const wide = await run(['verify', 'listing.json', '--aggregator', walrus.url, '--current-epoch', '100', '--support-epochs', '10']);
    expect(walrusLines(wide.err)).toEqual([expect.stringMatching(/WARNING \[walrus_epoch\]/)]);
    const narrow = await run(['verify', 'listing.json', '--aggregator', walrus.url, '--current-epoch', '102', '--support-epochs', '2']);
    expect(walrusLines(narrow.err)).toEqual([]);
  });

  it('verify_fails_after_expiry', async () => {
    await published('5');
    for (const current of ['105', '120']) {
      const result = await run(['verify', 'listing.json', '--aggregator', walrus.url, '--current-epoch', current]);
      expect(result.code).toBe(1);
      expect(walrusLines(result.err)).toEqual([expect.stringMatching(/FAILED \[walrus_epoch\] package: .* storage ended at epoch 105/)]);
    }
  });

  it('verify_estimates_current_epoch from the record', async () => {
    await published('5');
    // Stored at epoch 100 on Testnet (1-day epochs); three and a half days later is epoch 103.
    const again: string[] = [];
    await main(['verify', 'listing.json', '--aggregator', walrus.url], {
      stdout: () => {},
      stderr: (l) => again.push(l),
      cwd: dir,
      env: {},
      now: () => new Date(NOW.getTime() + 3.5 * DAY),
      verify: offline,
    });
    expect(walrusLines(again)).toEqual([expect.stringMatching(/WARNING \[walrus_epoch\] .* 2 epoch\(s\) from the current 103/)]);
  });

  it('verify_warns_without_record', async () => {
    await published();
    rmSync(join(dir, 'listing.walrus.json'));
    const result = await run(['verify', 'listing.json', '--aggregator', walrus.url]);
    expect(walrusLines(result.err)).toEqual([expect.stringMatching(/WARNING \[walrus_epoch\] .* no end epoch in the walrus record/)]);
    const other = await run(['verify', 'listing.json', '--aggregator', walrus.url, '--walrus-record', 'missing.json', '--current-epoch', '1']);
    expect(walrusLines(other.err)).toEqual([expect.stringMatching(/WARNING \[walrus_epoch\]/)]);
  });
});

describe('walrus helpers', () => {
  it('parses both store answers and refuses others', () => {
    expect(parseStoreAnswer({ newlyCreated: { blobObject: { blobId: 'b', registeredEpoch: 3, storage: { endEpoch: 9 } } } })).toEqual({
      blobId: 'b',
      endEpoch: 9,
      newlyCreated: true,
      registeredEpoch: 3,
    });
    expect(parseStoreAnswer({ alreadyCertified: { blobId: 'b', endEpoch: 9 } })).toEqual({ blobId: 'b', endEpoch: 9, newlyCreated: false });
    expect(() => parseStoreAnswer({ newlyCreated: { blobObject: { blobId: 'b' } } })).toThrow(/endEpoch/);
    expect(() => parseStoreAnswer({ error: 'x' })).toThrow(/did not store/);
  });

  it('epoch status: ok, expiring inside the window, expired at the end epoch', () => {
    expect(epochStatus(110, 100, 4).state).toBe('ok');
    expect(epochStatus(104, 100, 4).state).toBe('ok');
    expect(epochStatus(103, 100, 4).state).toBe('expiring');
    expect(epochStatus(100, 100, 4).state).toBe('expired');
  });

  it('estimates the epoch from the newest store that knows it', () => {
    const record: WalrusRecord = {
      schema: RECORD_SCHEMA,
      network: 'mainnet',
      blobs: [
        { blob_id: 'a', sha256: 'x', end_epoch: 20, stored_at: '2026-01-01T00:00:00Z', epoch_at_store: 10 },
        { blob_id: 'b', sha256: 'x', end_epoch: 30, stored_at: '2026-02-01T00:00:00Z', epoch_at_store: 12 },
        { blob_id: 'c', sha256: 'x', end_epoch: 30, stored_at: '2026-03-01T00:00:00Z' },
      ],
    };
    expect(estimateEpoch(record, new Date('2026-02-20T00:00:00Z'))).toBe(13);
    expect(estimateEpoch({ ...record, blobs: [record.blobs[2]!] }, new Date())).toBeUndefined();
  });
});
