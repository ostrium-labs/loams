# loams-walrus

The Walrus fast path (MS1 Task 4; decisions D1810 and D1811; plan
`docs/plans/2026-10-25-ms1-marketplace-seed.md`). It's a small HTTP client
for [Walrus](https://docs.wal.app) (Mysten Labs, Apache-2.0). Walrus is a
public, read-only, verifiable storage tier, and Loams uses it for three
things:

- public agent artifacts,
- a read-only cache of agent project knowledge,
- a build cache.

## What it is

One type, `Walrus`, built from the `[walrus]` config section.

- **Verified reads.** `read`, `read_quilt_patch`, `read_patch` and
  `read_to_path` take the expected SHA-256, which comes from a signed
  record. Every byte is checked against it before it is returned or cached.
  An aggregator is untrusted:
  - wrong bytes are dropped and logged with the aggregator's URL;
  - the next aggregator is tried;
  - if every aggregator sends wrong bytes, the read fails with
    `WalrusError::Integrity`.

  `read_to_path` streams into a temp file and renames it into place only
  after the file is verified.
- **Hedged reads.** A read starts on the first aggregator. If that request
  hasn't answered after `hedge_after_ms`, the next aggregator is asked too,
  and so on down the list. A failed request starts the next aggregator
  straight away. The first verified answer wins, and the other requests are
  cancelled.
- **Unverified range reads.** `read_range` returns `Unverified<Bytes>`.
  Walrus checks only the slivers a range read fetches, and a whole-blob
  SHA-256 can't vet a slice. Getting at the bytes takes
  `into_unverified_inner` or `verify(&sha256)`.
- **Quilts.** `read_quilt_patch` reads a patch by quilt id and identifier,
  `read_patch` reads one by patch id, and `list_patches` lists a quilt's
  patches. The listing isn't verified, so read the patches it names with a
  SHA-256 from a signed record.
- **Writes.** `store` and `store_quilt` go through a publisher.
  - A body over `max_blob_bytes` (10 MiB by default, the public endpoints'
    limit) is refused before upload with `WalrusError::TooLarge`.
  - Every write is read back through an aggregator, and its SHA-256
    checked, before `Ok`.
  - A `404` within `read_after_write_ms` (3 minutes by default) of this
    client's own write is retried with exponential backoff capped at 10 s.
    A CDN can cache a 404 just after certification, and public Testnet
    aggregators were seen to lag past 60 s (ruling 1 of PR #1232).
  - With no publisher configured, a write answers
    `WalrusError::NoPublisher`. This is always the case on Mainnet unless
    you configure your own publisher (Q1330).
- **The cache.** `VerifiedDiskCache` lives under `cache_dir`.
  - Blobs are keyed by SHA-256 (`sha256/<2>/<hex>`), never by blob id. An
    `index` file maps blob ids and patch ids to their SHA-256.
  - Writes are atomic: a temp file is written, synced, then renamed.
  - It is bounded by size, with least-recently-read eviction.
  - A read hits only for the expected SHA-256. Every read re-hashes the
    file, including the first read after a restart. A corrupt file is
    deleted, and the blob is fetched again.
- **Off by default.** With `enabled = false`, `Walrus::new` touches nothing
  (no cache directory, no token file, no network), and every call answers
  `WalrusError::Disabled`.

```toml
[walrus]
enabled = false                 # nothing is contacted while false
network = "testnet"             # "testnet" | "mainnet"; picks the defaults
aggregators = ["https://aggregator.walrus-testnet.walrus.space"]
publisher = "https://publisher.walrus-testnet.walrus.space"   # unset: the network's, if any
publisher_auth = ""             # file holding a bearer token for your own publisher
read_after_write_ms = 180000    # retry a 404 this long after a write
epochs = 5
hedge_after_ms = 300
request_timeout_ms = 30000
max_blob_bytes = 10485760
cache_dir = ""                  # WalrusConfig::with_data_dir: <data dir>/walrus-cache
cache_max_bytes = 4294967296
```

## What it is not

- **Not a `Store` backend.** The bucket stays the only source of truth (D1).
  Walrus holds public copies only. Losing every Walrus blob loses no data.
- **Not a Sui client.** It speaks HTTP only, to aggregators and publishers.
  It has no Sui SDK, no Walrus SDK, no `walrus-*` crates and no wallet, key
  or WAL token handling.
- **Not private.** Everything on Walrus is public and can't be made private
  later. Never store private repository content, private listings, secrets,
  tokens, user data or agent state.
- **Partly wired in.** Listing artifacts read their walrus locations
  through it (`loams-agentd-marketplace`'s `artifact` module, MS1 Task 5).
  Project knowledge and the build cache are MS1 Tasks 7 and 8.

## Endpoints (checked 2026-10-25)

| Use | Request |
| --- | --- |
| Read a blob | `GET <aggregator>/v1/blobs/<blob id>` |
| Read a range | the same route with `Range: bytes=<first>-<last>`, answered `206` (there is also `…/byte-range?start=&length=`, which this crate doesn't use) |
| Read a quilt patch | `GET <aggregator>/v1/blobs/by-quilt-id/<quilt id>/<identifier>` or `GET <aggregator>/v1/blobs/by-quilt-patch-id/<patch id>` |
| List a quilt's patches | `GET <aggregator>/v1/quilts/<quilt id>/patches` answers `[{"identifier","patch_id","tags"}]` |
| Store a blob | `PUT <publisher>/v1/blobs?epochs=<n>[&permanent=true]` answers `{"newlyCreated":{"blobObject":{"id","blobId","size","storage":{"endEpoch"},…},…}}` or `{"alreadyCertified":{"blobId","event","endEpoch"}}` |
| Store a quilt | `PUT <publisher>/v1/quilts?epochs=<n>[&permanent=true]`, `multipart/form-data` with one file field per identifier; it answers `{"blobStoreResult":…,"storedQuiltBlobs":[{"identifier","quiltPatchId"}]}` |

`publisher_auth` sends the file's token as `Authorization: Bearer <token>`.
This is **unverified against official docs; from community SDKs**:
docs.wal.app doesn't document the header an authenticated publisher
expects. The format comes from the iWalrusSDK blog and a Railway publisher
template.

Blobs are deletable by default since Walrus v1.33. The `deletable` flag is
deprecated, and this client never sends it.

Public endpoints:

- Testnet aggregator: `https://aggregator.walrus-testnet.walrus.space`
- Testnet publisher: `https://publisher.walrus-testnet.walrus.space`
- Mainnet aggregator: `https://aggregator.walrus-mainnet.walrus.space`
- Mainnet has no public publisher.

Testnet can be wiped without warning. Most public endpoints cap requests at
10 MiB.

Sources:

- HTTP API: <https://docs.wal.app/usage/web-api.html>
- Reading blobs: <https://docs.wal.app/docs/http-api/reading-blobs>
- Quilt HTTP APIs: <https://docs.wal.app/docs/http-api/quilt-http-apis>
- Range reads: <https://docs.wal.app/docs/http-api/streaming-media>
- Network reference: <https://docs.wal.app/docs/network-reference>
- The aggregator's and publisher's OpenAPI files,
  `crates/walrus-service/{aggregator,publisher}_openapi.yaml` in
  [MystenLabs/walrus](https://github.com/MystenLabs/walrus)

## Tests

The default tests use axum fakes of an aggregator and a publisher on
loopback. They make no network calls. One ignored test runs the whole
round trip on Testnet: it stores a blob and reads it back, does a range
read, stores a quilt, reads a patch and lists the quilt's patches.

```sh
LOAMS_TEST_WALRUS_TESTNET=1 cargo test -p loams-walrus --test testnet -- --ignored --nocapture
```
