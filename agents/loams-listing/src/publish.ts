// `loams-listing publish --walrus`: store a listing's artifact bytes on
// Walrus and write the `walrus` locations into the listing (plan MS1 Task 9,
// D1812). The bytes must be the ones the listing already pins, the store is
// read back and its SHA-256 checked before the listing changes, and the
// epochs the publisher reports go to the walrus record (ruling 2).
import { integrityOf } from './from-package.js';
import type { ArtifactLocation, Listing } from './types.js';
import {
  type Network,
  type ReadOptions,
  readVerified,
  recordBlob,
  sha256Hex,
  storeBlob,
  type WalrusRecord,
} from './walrus.js';

export class PublishError extends Error {}

/** What to store: the package file (`package`) or one archive file (its platform). */
export interface PublishInput {
  target: 'package' | { platform: string };
  bytes: Uint8Array;
}

export interface WalrusPublishOptions {
  publisher: URL;
  aggregator: URL;
  token?: string;
  network: Network;
  epochs: number;
  permanent: boolean;
  maxBytes: number;
  readAfterWriteMs: number;
  /** For tests: the read-back's first backoff. */
  backoffMs?: number;
  now?: () => Date;
}

export interface PublishedBlob {
  target: string;
  blobId: string;
  sha256: string;
  endEpoch: number;
  newlyCreated: boolean;
}

export interface PublishResult {
  listing: Listing;
  record: WalrusRecord;
  published: PublishedBlob[];
}

const MAX_LOCATIONS = 4;

function label(input: PublishInput): string {
  return input.target === 'package' ? 'package' : input.target.platform;
}

/** Refuses, before any request, bytes the listing does not pin or that are too large. */
function check(listing: Listing, input: PublishInput, maxBytes: number): void {
  const artifact = listing.artifact;
  if (!artifact) throw new PublishError(`${listing.id} has no artifact to publish`);
  const name = label(input);
  if (input.bytes.length > maxBytes) {
    throw new PublishError(`${name}: ${input.bytes.length} bytes is over the ${maxBytes}-byte limit (most public publishers cap a request at 10 MiB)`);
  }
  if (input.target === 'package') {
    if (artifact.type === 'archive') throw new PublishError('an archive artifact publishes its files: use --file <platform>=<file>');
    if (!artifact.integrity) throw new PublishError(`${artifact.name}: the listing pins no integrity to check the tarball against`);
    const actual = integrityOf(input.bytes);
    if (actual !== artifact.integrity) {
      throw new PublishError(`package: the tarball's integrity is ${actual}, not the listing's ${artifact.integrity}`);
    }
    return;
  }
  if (artifact.type !== 'archive') throw new PublishError(`a ${artifact.type} artifact publishes its package file: use --tarball <file>`);
  const { platform } = input.target;
  const file = artifact.files?.find((f) => f.platform === platform);
  if (!file) throw new PublishError(`the listing has no ${platform} file`);
  const actual = sha256Hex(input.bytes);
  if (actual !== file.sha256) throw new PublishError(`${platform}: the file's sha256 is ${actual}, not the listing's ${file.sha256}`);
}

function withLocation(locations: ArtifactLocation[] | undefined, location: ArtifactLocation, where: string): ArtifactLocation[] {
  const kept = (locations ?? []).filter((l) => !(l.kind === 'walrus' && l.sha256 === location.sha256));
  kept.push(location);
  if (kept.length > MAX_LOCATIONS) throw new PublishError(`${where}: more than ${MAX_LOCATIONS} locations`);
  return kept;
}

export async function publishWalrus(
  listing: Listing,
  inputs: PublishInput[],
  record: WalrusRecord | undefined,
  options: WalrusPublishOptions,
): Promise<PublishResult> {
  if (inputs.length === 0) throw new PublishError('nothing to publish: give --tarball or --file');
  for (const input of inputs) check(listing, input, options.maxBytes);
  if (record && record.network !== options.network) {
    throw new PublishError(`the walrus record is for ${record.network}, not ${options.network}`);
  }

  const out = structuredClone(listing);
  const artifact = out.artifact!;
  const published: PublishedBlob[] = [];
  const read: ReadOptions = {
    aggregator: options.aggregator,
    retry404Ms: options.readAfterWriteMs,
    maxBytes: options.maxBytes,
    backoffMs: options.backoffMs,
  };
  let next = record;
  for (const input of inputs) {
    const name = label(input);
    const sha256 = sha256Hex(input.bytes);
    const stored = await storeBlob(input.bytes, {
      publisher: options.publisher,
      token: options.token,
      epochs: options.epochs,
      permanent: options.permanent,
    });
    // Read back through an aggregator before the listing names the blob.
    try {
      await readVerified(stored.blobId, sha256, read);
    } catch (error) {
      throw new PublishError(`${name}: the read-back of ${stored.blobId} failed, so the listing is not changed: ${(error as Error).message}`);
    }
    const location: ArtifactLocation = { kind: 'walrus', blob_id: stored.blobId, sha256 };
    if (input.target === 'package') {
      artifact.locations = withLocation(artifact.locations, location, '/artifact/locations');
    } else {
      const { platform } = input.target;
      const file = artifact.files!.find((f) => f.platform === platform)!;
      file.locations = withLocation(file.locations, location, `${platform} locations`);
    }
    const epochAtStore = stored.newlyCreated ? (stored.registeredEpoch ?? stored.endEpoch - options.epochs) : undefined;
    next = recordBlob(next, options.network, {
      blob_id: stored.blobId,
      sha256,
      end_epoch: stored.endEpoch,
      stored_at: (options.now?.() ?? new Date()).toISOString(),
      ...(epochAtStore !== undefined && epochAtStore >= 0 ? { epoch_at_store: epochAtStore } : {}),
    });
    published.push({ target: name, blobId: stored.blobId, sha256, endEpoch: stored.endEpoch, newlyCreated: stored.newlyCreated });
  }
  return { listing: out, record: next!, published };
}
