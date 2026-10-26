// loams-listing: lint <listing.json>… | from-package <dir> | index build <dir> --out <dir> | verify | publish --walrus
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fromPackageDir, PackageError } from './from-package.js';
import { buildIndex, canonical, IndexError, writeIndex } from './index-build.js';
import { formatIssue, hasErrors, lint, RULES } from './lint.js';
import type { Listing } from './types.js';
import { publishWalrus, PublishError, type PublishInput } from './publish.js';
import { httpFetcher, sigstoreVerifier, verifyListing, type VerifyOptions, type VerifyReport, type WalrusVerifyOptions } from './verify.js';
import {
  checkEndpoint,
  DEFAULT_EPOCHS,
  DEFAULT_MAX_BYTES,
  DEFAULT_READ_AFTER_WRITE_MS,
  MAX_EPOCHS,
  type Network,
  readRecord,
  recordPathFor,
  WalrusError,
  writeRecord,
} from './walrus.js';

export interface Io {
  stdout: (line: string) => void;
  stderr: (line: string) => void;
  cwd: string;
  /** For tests: replaces the network and Sigstore's public-good instance. */
  verify?: VerifyOptions;
  /** The environment (`LOAMS_WALRUS_*`); `process.env` when absent. */
  env?: Record<string, string | undefined>;
  /** For tests: the clock, and the read-back's first backoff. */
  now?: () => Date;
  backoffMs?: number;
}

const USAGE = `usage:
  loams-listing lint [--first-party-only] <listing.json>…
  loams-listing from-package <dir> (--tarball <file> | --integrity <sha512-…>) [--commit <sha>] [--out <file>]
  loams-listing index build <listings-dir> --out <dir> [--first-party-only] [--source-commit <sha>]
  loams-listing verify <listing.json>… [--save-provenance <dir>] [--report <file.json>]
      [--aggregator <url>] [--walrus-record <file>] [--current-epoch <n>] [--support-epochs <n>]
  loams-listing publish --walrus <listing.json> (--tarball <file> | --file <platform>=<file>…)
      --publisher <url> --aggregator <url> --network testnet|mainnet [--publisher-token-file <file>]
      [--epochs <n>] [--permanent] [--max-bytes <n>] [--read-after-write-ms <n>] [--out <file>]
  loams-listing rules

  The publisher, aggregator and token file may also come from LOAMS_WALRUS_PUBLISHER,
  LOAMS_WALRUS_AGGREGATOR and LOAMS_WALRUS_PUBLISHER_TOKEN_FILE. There is no default publisher.`;

interface Parsed {
  positional: string[];
  flags: Map<string, string | true>;
  /** Flags given more than once (`--file`). */
  multi: Map<string, string[]>;
}

const VALUE_FLAGS = new Set([
  '--tarball',
  '--integrity',
  '--commit',
  '--out',
  '--source-commit',
  '--save-provenance',
  '--report',
  '--file',
  '--publisher',
  '--aggregator',
  '--publisher-token-file',
  '--network',
  '--epochs',
  '--max-bytes',
  '--read-after-write-ms',
  '--walrus-record',
  '--current-epoch',
  '--support-epochs',
]);

function parseArgs(args: string[]): Parsed {
  const positional: string[] = [];
  const flags = new Map<string, string | true>();
  const multi = new Map<string, string[]>();
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (VALUE_FLAGS.has(arg)) {
      const value = args[++i];
      if (value === undefined) throw new UsageError(`${arg} needs a value`);
      flags.set(arg, value);
      multi.set(arg, [...(multi.get(arg) ?? []), value]);
    } else if (arg.startsWith('--')) {
      flags.set(arg, true);
    } else {
      positional.push(arg);
    }
  }
  return { positional, flags, multi };
}

class UsageError extends Error {}

function str(flags: Parsed['flags'], name: string): string | undefined {
  const value = flags.get(name);
  return typeof value === 'string' ? value : undefined;
}

function int(flags: Parsed['flags'], name: string, fallback: number, min = 0, max = Number.MAX_SAFE_INTEGER): number {
  const raw = str(flags, name);
  if (raw === undefined) return fallback;
  if (!/^\d+$/.test(raw)) throw new UsageError(`${name} needs a whole number, not ${raw}`);
  const value = Number(raw);
  if (value < min || value > max) throw new UsageError(`${name} must be between ${min} and ${max}`);
  return value;
}

function walrusVerifyOptions(flags: Parsed['flags'], env: Record<string, string | undefined>, at: (p: string) => string, listingFile: string, io: Io): WalrusVerifyOptions {
  const aggregator = str(flags, '--aggregator') ?? env.LOAMS_WALRUS_AGGREGATOR;
  const recordFile = str(flags, '--walrus-record');
  const current = str(flags, '--current-epoch');
  return {
    aggregator: aggregator ? checkEndpoint('--aggregator', aggregator) : undefined,
    record: readRecord(recordFile ? at(recordFile) : recordPathFor(at(listingFile))),
    currentEpoch: current === undefined ? undefined : int(flags, '--current-epoch', 0),
    supportEpochs: int(flags, '--support-epochs', 4),
    maxBytes: int(flags, '--max-bytes', 512 * 1024 * 1024, 1),
    now: io.now,
  };
}

export async function main(args: string[], io: Io): Promise<number> {
  try {
    const [command, ...rest] = args;
    const { positional, flags, multi } = parseArgs(rest);
    const at = (path: string) => resolve(io.cwd, path);
    const env = io.env ?? process.env;
    switch (command) {
      case 'lint': {
        if (positional.length === 0) throw new UsageError('lint needs at least one listing.json');
        let failed = false;
        for (const file of positional) {
          let doc: unknown;
          try {
            doc = JSON.parse(readFileSync(at(file), 'utf8'));
          } catch (error) {
            io.stderr(`${file}: error [schema] not readable JSON: ${(error as Error).message}`);
            failed = true;
            continue;
          }
          const issues = lint(doc, { firstPartyOnly: flags.has('--first-party-only') });
          for (const issue of issues) io.stderr(formatIssue(file, issue));
          if (hasErrors(issues)) failed = true;
          else io.stdout(`${file}: ok`);
        }
        return failed ? 1 : 0;
      }
      case 'from-package': {
        const [dir] = positional;
        if (!dir) throw new UsageError('from-package needs a package directory');
        const tarball = str(flags, '--tarball');
        const listing = fromPackageDir(at(dir), {
          commit: str(flags, '--commit'),
          integrity: str(flags, '--integrity'),
          tarball: tarball ? at(tarball) : undefined,
        });
        const issues = lint(listing);
        for (const issue of issues) io.stderr(formatIssue('listing.json', issue));
        const text = `${JSON.stringify(listing, null, 2)}\n`;
        const out = str(flags, '--out');
        if (out) writeFileSync(at(out), text);
        else io.stdout(text.trimEnd());
        return hasErrors(issues) ? 1 : 0;
      }
      case 'index': {
        const [sub, dir] = positional;
        if (sub !== 'build' || !dir) throw new UsageError('index build needs a listings directory');
        const out = str(flags, '--out');
        if (!out) throw new UsageError('index build needs --out <dir>');
        const result = buildIndex(at(dir), {
          firstPartyOnly: flags.has('--first-party-only'),
          sourceCommit: str(flags, '--source-commit'),
        });
        for (const warning of result.warnings) io.stderr(warning);
        for (const file of writeIndex(result, at(out))) io.stdout(`wrote ${file}`);
        return 0;
      }
      case 'verify': {
        if (positional.length === 0) throw new UsageError('verify needs at least one listing.json');
        if (str(flags, '--walrus-record') && positional.length > 1) throw new UsageError('--walrus-record takes one listing');
        const base = io.verify ?? { fetcher: httpFetcher(), verifier: await sigstoreVerifier() };
        const reports: (VerifyReport & { file: string })[] = [];
        for (const file of positional) {
          const listing = JSON.parse(readFileSync(at(file), 'utf8')) as Listing;
          const options = { ...base, walrus: walrusVerifyOptions(flags, env, at, file, io) };
          const report = { file, ...(await verifyListing(listing, options)) };
          reports.push(report);
          for (const check of report.checks) {
            const state = !check.ok ? 'FAILED' : check.warning ? 'WARNING' : 'ok';
            (check.ok && !check.warning ? io.stdout : io.stderr)(`${file}: ${state} [${check.check}] ${check.detail}`);
          }
          const save = str(flags, '--save-provenance');
          if (save && report.ok) {
            const dir = resolve(at(save), listing.id, listing.version);
            mkdirSync(dir, { recursive: true });
            report.bundles.forEach((b, i) => writeFileSync(resolve(dir, `provenance-${i}.sigstore.json`), canonical(b.bundle)));
          }
        }
        const reportFile = str(flags, '--report');
        if (reportFile) writeFileSync(at(reportFile), canonical(reports.map(({ bundles: _bundles, ...rest }) => rest)));
        return reports.every((r) => r.ok) ? 0 : 1;
      }
      case 'publish': {
        if (!flags.has('--walrus')) throw new UsageError('publish needs --walrus (the only place it publishes to)');
        const [file] = positional;
        if (!file || positional.length > 1) throw new UsageError('publish --walrus needs one listing.json');
        const publisher = str(flags, '--publisher') ?? env.LOAMS_WALRUS_PUBLISHER;
        if (!publisher) throw new UsageError('publish --walrus needs --publisher <url> or LOAMS_WALRUS_PUBLISHER (there is no default publisher)');
        const aggregator = str(flags, '--aggregator') ?? env.LOAMS_WALRUS_AGGREGATOR;
        if (!aggregator) throw new UsageError('publish --walrus needs --aggregator <url> or LOAMS_WALRUS_AGGREGATOR, to read the blob back');
        const network = str(flags, '--network');
        if (network !== 'testnet' && network !== 'mainnet') throw new UsageError('publish --walrus needs --network testnet|mainnet');
        const tokenFile = str(flags, '--publisher-token-file') ?? env.LOAMS_WALRUS_PUBLISHER_TOKEN_FILE;
        const tarball = str(flags, '--tarball');
        const files = multi.get('--file') ?? [];
        if (tarball && files.length > 0) throw new UsageError('give --tarball or --file, not both');
        const inputs: PublishInput[] = [];
        if (tarball) inputs.push({ target: 'package', bytes: readFileSync(at(tarball)) });
        for (const spec of files) {
          const eq = spec.indexOf('=');
          if (eq <= 0 || eq === spec.length - 1) throw new UsageError(`--file needs <platform>=<file>, not ${spec}`);
          inputs.push({ target: { platform: spec.slice(0, eq) }, bytes: readFileSync(at(spec.slice(eq + 1))) });
        }
        if (inputs.length === 0) throw new UsageError('publish --walrus needs --tarball <file> or --file <platform>=<file>');
        const listingPath = at(file);
        const recordFile = str(flags, '--walrus-record');
        const out = str(flags, '--out');
        const listingOut = out ? at(out) : listingPath;
        const recordOut = recordFile ? at(recordFile) : recordPathFor(listingOut);
        const listing = JSON.parse(readFileSync(listingPath, 'utf8')) as Listing;
        const result = await publishWalrus(listing, inputs, readRecord(recordFile ? at(recordFile) : recordPathFor(listingPath)), {
          publisher: checkEndpoint('--publisher', publisher),
          aggregator: checkEndpoint('--aggregator', aggregator),
          token: tokenFile ? readFileSync(at(tokenFile), 'utf8').trim() : undefined,
          network: network as Network,
          epochs: int(flags, '--epochs', DEFAULT_EPOCHS, 1, MAX_EPOCHS),
          permanent: flags.has('--permanent'),
          maxBytes: int(flags, '--max-bytes', DEFAULT_MAX_BYTES, 1),
          readAfterWriteMs: int(flags, '--read-after-write-ms', DEFAULT_READ_AFTER_WRITE_MS),
          backoffMs: io.backoffMs,
          now: io.now,
        });
        for (const p of result.published) {
          io.stdout(`stored ${p.target} ${p.blobId} end_epoch ${p.endEpoch} (${p.newlyCreated ? 'newly created' : 'already certified'})`);
        }
        const issues = lint(result.listing);
        for (const issue of issues) io.stderr(formatIssue(file, issue));
        if (hasErrors(issues)) {
          io.stderr(`${file}: not written: the listing with its walrus locations does not lint`);
          return 1;
        }
        writeFileSync(listingOut, `${JSON.stringify(result.listing, null, 2)}\n`);
        writeRecord(recordOut, result.record);
        io.stdout(`wrote ${listingOut}`);
        io.stdout(`wrote ${recordOut}`);
        return 0;
      }
      case 'rules': {
        for (const [rule, text] of Object.entries(RULES)) io.stdout(`${rule}\t${text}`);
        return 0;
      }
      default:
        throw new UsageError(command ? `unknown command ${command}` : 'no command');
    }
  } catch (error) {
    if (error instanceof UsageError) {
      io.stderr(`${error.message}\n${USAGE}`);
      return 2;
    }
    if (error instanceof IndexError) {
      for (const problem of error.problems) io.stderr(problem);
      return 1;
    }
    if (error instanceof PublishError || error instanceof WalrusError) {
      io.stderr(error.message);
      return 1;
    }
    if (error instanceof PackageError) {
      io.stderr(error.message);
      return 1;
    }
    throw error;
  }
}

export { canonical };
