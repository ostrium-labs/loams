# Golden fixtures (PG2 Task 31)

Recorded 2026-10-08.

- `wal-17.bin.zst`: a Postgres 17 WAL range, zstd-compressed (`zstd -19`, 687 KB; 3.7 MB raw). The header is `start_lsn` (u64), `pg_version` (u32) and `system_id` (u64), all big-endian, followed by the WAL bytes.
  - It covers `0/4000028..0/4396768`, about 3.7 MB, a little over 27 `MAX_SEND_SIZE` reads.
  - It was taken from `postgres:17.11` (server version 170011) running this workload, which produced about 22k records (`pg_waldump`: Heap, Btree, Heap2, Standby, Transaction, Storage, XLOG, LogicalMessage, Database):
    - a table with a primary key and a secondary index;
    - 8000 inserts, then updates and deletes;
    - an aborted transaction;
    - `CREATE DATABASE ... STRATEGY file_copy`;
    - `VACUUM`;
    - a TOAST table, then `TRUNCATE`;
    - `pg_logical_emit_message`.
  - The range starts after `pg_switch_wal()`, at the first record of segment 4.
- `fork-sender-17-unsharded.sha256` and `fork-sender-17-shard1of2.sha256`: one line per interpreted CopyData body that Neon's safekeeper sent for `START_REPLICATION PHYSICAL 0/4000028`. Each line holds `streaming_lsn` and `commit_lsn` (hex), the body's length and its sha256. Keepalives are left out. The tests require every line to match exactly, which is byte identity of every body.
  - The request carried `protocol={"type":"interpreted","args":{"format":"protobuf","compression":{"zstd":{"level":1}}}}`.
  - The shard options were `shard_count=0 shard_number=0 shard_stripe_size=2048` for the unsharded file, and `shard_count=2 shard_number=1 shard_stripe_size=2048` for the shard 1 of 2 file.

## Which sender

The safekeeper was the one in `ghcr.io/neondatabase/neon@sha256:ead56a7b…`, which reports `git-env:77e22e4bf09d88b70b4a83a38c2f6de6301816b4`. That commit is an ancestor of the fork's pinned `fa504217`. None of the 11 commits between them touch:
- `libs/wal_decoder`;
- `libs/postgres_ffi/src`;
- `libs/pq_proto`;
- `libs/utils/src/postgres_client.rs`;
- the shard types;
- `safekeeper/src/send_interpreted_wal.rs`;
- `safekeeper/src/wal_reader_stream.rs`.

The only change to the fork's `Cargo.lock` in that span is the GCS provider. Loams' trim commit on top of `fa504217` (`1218fb7a`, branch `loams/decoder-trim`) changes only which dependencies are linked, not the decoder. This workspace's lock uses the fork's zstd (1.5.5, `zstd-sys 2.0.9`) and `async-compression` (0.4.5), so compression is byte-identical. Recording twice gives identical files.

## Byte identity needs a static commit

The fork's reader takes the readable end (`commit_lsn`) once and keeps it until it has caught up; `loams-wal` re-reads it before every chunk. Both cut chunks at `MAX_SEND_SIZE` back to a page boundary unless the chunk reaches that end. The streams are therefore identical when the commit does not move while the reader catches up, as here (the whole range was committed before the reader connected). With a moving commit, the chunk boundaries and the `commit_lsn` fields may differ while the records stay the same.

## Re-recording

Run `crates/loams-safekeeper/examples/record_interpreted.rs` against a fresh safekeeper:

```text
podman run -d --network host --entrypoint /bin/sh <neon image> -c \
  "mkdir -p /tmp/sk && exec safekeeper --listen-pg=127.0.0.1:55454 \
   --listen-http=127.0.0.1:57676 --id=1 -D /tmp/sk --broker-endpoint=http://127.0.0.1:1"
zstd -d wal-17.bin.zst -o wal-17.bin
cargo run -p loams-safekeeper --features server --example record_interpreted -- \
  127.0.0.1:55454 wal-17.bin fork-sender-17-unsharded.sha256
cargo run -p loams-safekeeper --features server --example record_interpreted -- \
  127.0.0.1:55454 wal-17.bin fork-sender-17-shard1of2.sha256 2 1 2048   # a fresh safekeeper
```

The recorder pushes the range as term 1 and commits it, using `loams_safekeeper::propose`. It then reads the range back as the pageserver does.
