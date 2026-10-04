# FL2 — the chDB dependency spike

Task 0 of [the FL2 plan](2026-10-01-fl2-house-sql.md): measure chDB before linking it. Recorded 2026-10-04 on `fl2-t0-chdb-spike`.

The plan's Task 0 asks for eleven measurements. Eight are answered here from the pinned artefacts themselves; three need FL1's running stack and are recorded as outstanding with the task that owns each. Every digest below was read from the release's own `SHA256SUMS` and re-verified against the bytes this spike downloaded.

## 1. The pin

| What | Value |
|---|---|
| Repository | **`chdb-io/chdb-core`**, not `clickhouse/chdb-core` — the plan does not name it, and `chdb-core` is not a crates.io crate (checked 2026-10-04: `chdb-core` and `libchdb` are both absent; the `chdb` crate at 0.1.2 is an unrelated third-party binding) |
| Release | **v26.9.0**, published 2026-09-28, the latest 26.9.x and `Latest` on the repo |
| ClickHouse version | carried by chDB v26.9.0; **not yet read through the C ABI** — see §3, this is outstanding because the dlopen attempt segfaulted on a guessed signature and the header then proved the guess wrong |
| `linux-x86_64-libchdb.tar.gz` | `c6398bcc71dc58d12fb81548540aacd5d8248830ec81cec537f51c114670543b`, 180 541 166 bytes |
| `linux-aarch64-libchdb.tar.gz` | `070116864cde6fdb3276fb1b28702b3b002da42038bb59c99e2a1360dd7f31ed` |
| `linux-x86_64-libchdb-static.tar.gz` | `dfce17eb6119bc58d0a2d067b14211113437697f50be286a8a73cc54941101f1` (recorded only; Ruling 2 links dynamically) |

The digest was verified the hard way: a plain `curl` truncated at 76 MB and produced a **mismatching** digest, and the file only verified after three resumed transfers. `build.rs` must therefore fail closed on a digest mismatch and CI should cache by digest, not by tag — a tag is not an immutable reference.

`libchdb.so` unpacks to a single **554 MB** stripped ELF shared object, which is why Ruling 2 forbids the static archive and why the container ships the `.so` beside the binary.

## 2. The header is not in the release

**Finding that changes Task 1.** The tarball contains exactly one file, `libchdb.so` — no header. `bindgen` needs `chdb.h`, so the pinned header must come from the repository at the same tag. It is **`programs/local/chdb.h`** at tag `v26.9.0`: **1 176 lines, 75 `chdb_` declarations**. This spike vendors it to `fabric/crates/loams-chdb-sys/chdb.h` so the header and the `.so` cannot drift apart — Ruling 2's digest then covers the binary while the vendored header is covered by review.

## 3. The C ABI, and a signature the plan's shape would have got wrong

**52 exported `chdb_` symbols**, all present. The ABI is the **2.x "stable" shape**, and this matters:

```c
CHDB_EXPORT const char *      chdb_version(void);
CHDB_EXPORT chdb_connection * chdb_connect(int argc, char ** argv);   // NOT a path string
CHDB_EXPORT void              chdb_close_conn(chdb_connection * conn);
CHDB_EXPORT chdb_result *     chdb_query(chdb_connection, const char * query, const char * format);  // no out-param error
CHDB_EXPORT const char *      chdb_result_buffer(chdb_result *);
CHDB_EXPORT size_t            chdb_result_length(chdb_result *);
CHDB_EXPORT const char *      chdb_result_error(chdb_result *);
```

A first attempt to drive the library through hand-written `ctypes` bindings, guessing `chdb_connect(const char * path)` and a `char **error` out-parameter — the 1.x shape — **segfaulted**. That is the concrete justification for Ruling 1: Loams binds `chdb.h` with `bindgen` and never hand-writes a prototype. There is no "mostly right" FFI here; there is exactly the header or a crash.

Every operation Task 1's `Produces` block names maps to a real symbol:

| Task 1 operation | Symbols |
|---|---|
| connect / close | `chdb_connect`, `chdb_close_conn`, `chdb_shutdown` |
| buffered query | `chdb_query`, `chdb_query_n`, `chdb_query_cmdline` |
| parameterized query | `chdb_query_with_params`, `chdb_query_with_params_n`, `chdb_stream_query_with_params(_n)`, `chdb_stream_insert_with_params(_n)`, `chdb_stream_query_arrow_with_params(_n)` |
| streaming query + fetch | `chdb_stream_query(_n)`, `chdb_stream_fetch_result`, `chdb_streaming_fetch_result` |
| result buffer | `chdb_result_buffer`, `chdb_result_length` |
| error | `chdb_result_error`, `chdb_streaming_result_error`, `chdb_stream_insert_error` |
| rows / bytes read | `chdb_result_rows_read`, `chdb_result_bytes_read`, `chdb_result_storage_rows_read`, `chdb_result_storage_bytes_read`, `chdb_result_rows_written`, `chdb_result_bytes_written` |
| elapsed | `chdb_result_elapsed` |
| **Arrow input** | `chdb_insert_arrow_stream`, `chdb_insert_arrow_array`, `chdb_arrow_scan`, `chdb_arrow_array_scan`, `chdb_arrow_unregister_table` |
| **Arrow output** | `chdb_query_arrow`, `chdb_query_arrow_n`, `chdb_stream_query_arrow(_n)(_with_params(_n))`, `chdb_stream_fetch_arrow` |
| **query cancellation** | `chdb_streaming_cancel_query`, `chdb_stream_cancel_query`, `chdb_stream_cancel_insert` |
| **declining signal handlers** | `chdb_set_signal_handlers_enabled`, `chdb_reset_signal_handlers` |
| destroy | `chdb_destroy_query_result`, `chdb_destroy_result`, `chdb_destroy_insert_stream` |

**No gaps.** Task 0's fallback — "for Arrow input without an ABI call: write the tail as an Arrow IPC file in the temp directory and read it with `file()`" — is **not needed**. Arrow input, Arrow output, cancellation, signal-handler control and parameterised queries are all first-class ABI calls. That removes the plan's most unpleasant contingency and one of the reasons the deny list had to be so careful about `file()`.

Also present and unmentioned by the plan: `chdb_stream_insert(_n)`, `chdb_stream_append`, `chdb_stream_done`, `chdb_backup_database_n`, `chdb_restore_database_n`, and **`chdb_classify_query_n`**.

## 4. `chdb_classify_query_n` — a finding for Ruling 4

Ruling 4 chooses sqlparser 0.63's `ClickHouseDialect` to classify the statements Loams owns. But chDB ships ClickHouse's **own** query classifier behind `chdb_classify_query_n`, and ClickHouse is by construction exact where sqlparser is approximate.

This does not overturn Ruling 4, and the reason is worth recording: Loams must not merely classify a statement, it must **map** the DDL it owns onto Fluss — extracting engine, partition key, sorting key, column types and TTL. A classifier that answers "this is a CREATE TABLE" cannot produce that mapping, so sqlparser (or a parser over the AST) stays necessary. What `chdb_classify_query_n` is good for is the *negative* case: deciding that a statement is **not** one Loams owns and therefore belongs to chDB unchanged, where ClickHouse's own answer is authoritative and sqlparser's is a guess. Task 4 should use it for that half and sqlparser for the half Loams must understand, and the surface page should say which half decided each statement.

## 5. Outstanding, and who owns each

| Task 0 item | Why it is outstanding | Owner |
|---|---|---|
| ClickHouse version via `SELECT version()` | The ctypes attempt segfaulted on a guessed signature. Reading it needs the real bindings, which is Task 1's first test | Task 1 (`engine.rs`) |
| Reference `clickhouse-server` image digest | Must match the version chDB carries, which is the row above | Task 1, then Task 10 |
| Iceberg from chDB (`icebergS3`, `iceberg_metadata_file_path`, `iceberg_snapshot_id`) | Needs FL1 Task 9's Lakekeeper tables and the FL1 stack | FL1 Task 9, verified in FL2 Task 7 |
| `fluss-rs` lake-snapshot offsets, log scan from offsets, PK changelog with row kinds, write acknowledgements | FL1 does not exist. `fabric/` exists only because CN1 Task 1 had to create it to hold `loams-flow` (CN1 Ruling 1) | FL1 Task 0's gap table, consumed by FL2 Task 7 |
| sqlparser 0.63 parsing 120 DDL forms | The corpus is Task 10's to seed | Task 10 |
| Driver request captures (Rust, Python, Go, Java) | Needs the reference server and a logging proxy | Task 12 |
| libchdb load time and idle RSS; RSS under 8 concurrent ClickBench queries | Needs the library linked and a working engine | Task 1, then Task 13 |
| ClickBench `hits_{0..9}.parquet` SHA-256s and row counts | ~10 M rows across ten files. D414 forbids vendoring the queries, and the dataset's own terms are checked in the same pass | Task 11 |

## 6. One constraint worth surfacing now

**One cargo build at a time**, and **the FL1 stack stopped during builds** (Global Constraints) — both because a 554 MB `libchdb.so` link plus a full ClickHouse symbol table is expensive. The second is moot today: there is no FL1 stack to stop. It matters again once FL1 lands, and it should be recorded in FL1's spike so the two plans agree on the build discipline rather than discovering it as a CI timeout.