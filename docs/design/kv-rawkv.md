# KV on TiKV's RawKV API

**Status: built; post-0.1.4.** This note records the coordinator's decision of
2026-10-30, taken under the owner's "do whatever is best" and the defaults
rule. The rulings are K2-16 to K2-19 in
[the KV plan](../plans/2026-10-21-kv1-kv-gateways.md#rulings-made-during-execution).

## Why

A `loams_kv::Store` runs every operation as a Percolator transaction, even a
one-key `GET`. So each operation first takes a timestamp from PD's TSO, and PD
is on every hot path. In the BQ1 combined run, the kv engine's TiKV op ran
TSO-latency bound: about 4k QPS at depth 16, with a 2 s p99, on a TiKV core
that was 0.41 busy. The owner asked for TiKV's RawKV API v2, keyspace-aware,
for the KV engine's non-transactional operations, keeping transactions only
where several keys must change together.

## The constraint that shapes it

API v2 encodes the mode in every key, with a raw prefix and a transactional
prefix inside the keyspace. Data written through RawKV is invisible to TxnKV,
and the other way round. So an engine cannot send some operations on a key raw
and others transactional: **a key lives in one mode for its whole life**.

The RESP engine keeps a meta record per key (type, version, expiry), an expiry
index, a GC queue and leases. `SET` over a hash, `EXPIRE` and `DEL` with GC
are all multi-key atomic. That rules out moving "some ops" to raw inside a
transactional database.

## The decision

| Part | What |
|---|---|
| **C** | `loams_kv::RawStore`: get, batch_get, put (with an optional TTL), batch_put, delete, batch_delete, compare_and_swap, ttl and scan, on TiKV's RawKV API v2, with an in-memory backend for tests. There are no transactions. |
| **A** | `raw_databases = [n, …]` in the RESP gateway's config: those databases serve strings only, on the raw store. The default stays transactional. |
| **B-lite** | Batched TSO for the transactional databases. The pinned client (`client-rust` `41b3588`) already coalesces concurrent TSO requests into one PD round trip, up to 64 per batch (`pd/timestamp.rs`). There is no batch-wait setting, so there is nothing to configure. There is no cached read timestamp either, because that would allow stale reads. |

### Which operations are raw and which are transactional

| Where | Raw (`RawStore`) | Transactional (`Store`) |
|---|---|---|
| A database in `raw_databases` | `GET`, `MGET`, `STRLEN`, `EXISTS`, `TYPE`, `TTL`, `PTTL`, `SET` (plain and with `EX`/`PX`/`EXAT`/`PXAT`), `SETEX`, `PSETEX`, `MSET` (not atomic), `SET NX`/`SETNX` (CAS from absent), `SET XX`/`SET … GET`/`GETSET` (CAS loop), `INCR`/`DECR`/`INCRBY`/`DECRBY` (CAS loop), `DEL`/`UNLINK` (count read first) | nothing |
| Any other database | nothing | every command, as before |

A raw database refuses, with an error that names the database:
- hashes, sets, sorted sets and lists;
- `KEYS`, `SCAN`, `DBSIZE` and `FLUSHDB`;
- `EXPIRE`, `PERSIST` and `SET KEEPTTL`;
- `SET NX`, `XX` or `GET` combined with an expiry;
- `INCR` on a key that has a TTL;
- anything multi-key atomic.

`FLUSHALL` is refused whenever any raw database is configured, because the raw
keys would survive it.

### Rules the raw store keeps

- **Every write is atomic-mode.** TiKV's CAS is linearizable only if every
  write to the key takes the same latch. So `put`, `batch_put` and `delete` use
  the client's atomic mode, while reads use the plain client. Atomic mode
  refuses TiKV's batch delete, so `batch_delete` deletes one key at a time,
  concurrently.
- **CAS carries no TTL.** The client's CAS request has no TTL, so a swap
  clears the key's TTL, and TiKV has no way to change a TTL in place. That is
  why the RESP raw mode refuses what would lose a TTL silently, instead of
  doing it racily. A plain `SET` drops the TTL in Valkey too, so `SET XX`
  matches Valkey.
- **TTLs are whole seconds,** rounded up, and need
  `storage.enable-ttl = true` on the cluster. `bench/all` runs its TiKV that
  way.
- **Batches are not atomic** across regions.
- **Keys:** `root ‖ key`. For the RESP raw mode the root is the Redis product
  root, and the key is `db ‖ user key`.

## Numbers (measured)

The harness was `kv-native` (`bench/kv/native`) on the `bench/all` kv cluster:
one PD and one TiKV on core 9, the load generator on core 7, 64 requests in
flight, 20 s windows, 100,000 keys of 64 bytes, 2026-10-30. Every step started
below 70 °C (60–69 °C).

| Operation | Depth | Transactional (`Store`) | Raw | Raw / txn |
|---|---:|---:|---:|---:|
| put | 1 | 4,017/s (p50 12.3 ms, p99 63 ms) | 13,860/s (p50 3.3 ms, p99 19 ms) | 3.5× |
| put | 16 | 41,703 keys/s | 76,341 keys/s | 1.8× |
| get | 1 | 33,344/s (p50 1.9 ms, p99 3.2 ms) | 47,491/s (p50 1.3 ms, p99 2.6 ms) | 1.4× |
| get | 16 | 161,075 keys/s | 206,089 keys/s | 1.3× |

`kv-native`'s raw puts use the plain client. `RawStore` writes in atomic mode
(see the rules above), which TiKV documents as more expensive. So the raw put
rows are an upper bound for `RawStore`. Its own numbers will come from the
combined run once the kv slot drives it.

## Follow-ups

- A CAS with a TTL in the `client-rust` fork (TiKV's `RawCasRequest` has a
  `ttl` field) would let a raw database serve `INCR` on an expiring key and
  `SET NX EX`.
- `SCAN` and `KEYS` over a raw database: a raw scan of `db ‖ …`, not atomic.
