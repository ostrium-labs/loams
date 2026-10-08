# Captured client handshakes

These are real MySQL client handshakes against keyspace-mode TiDB v8.5.8 (`pingcap/tidb@sha256:df168c76…`, spike stack). They were captured on 2026-10-09 with `scripts/sqlgate/capture/capture.sh`, through its recording proxy (`proxy.py`).

**Format.** Each `*.jsonl` file holds one MySQL packet payload per line, with `dir` (`s2c` or `c2s`), `seq` and `hex`.

**The user.** Every capture logs in as `loams_cap` with the test password `capture`, created `IDENTIFIED WITH caching_sha2_password`. The password is a fixed test value, not a secret.

**What each capture holds:**
1. TiDB's greeting (plugin `mysql_native_password`).
2. The client's `HandshakeResponse41` (a native scramble).
3. TiDB's `AuthSwitchRequest` to `caching_sha2_password`.
4. The client's SHA-2 scramble, which `tests/it/clients.rs` reproduces from the password and nonce.
5. `AuthMoreData` asking for full authentication.
6. The client's request for the RSA key (`0x02`).
7. TiDB's ERR 1045. The connection is plaintext and TiDB has no RSA key, so this is expected.

**`*-ssl.jsonl` files.** The proxy rewrites the greeting to advertise `CLIENT_SSL` and records the client's next packet:
- `mysql84` and `connector-j` answer with an `SSLRequest`;
- `mysql2` (no `ssl` option) and `mariadb` (`--skip-ssl`) answer with a plaintext response.

| File | Client | Version | Source |
|---|---|---|---|
| `mysql84` | `mysql` (libmysql) | 8.4.10 | `docker.io/library/mysql@sha256:02aa6476f6b675e5d4d19c5b437798b1b8a4048ae39383eac609814998ed15b8` |
| `connector-j` | MySQL Connector/J | 9.7.0 | `mysql-connector-j-9.7.0.jar`, OpenJDK 25 |
| `mysql2` | Node-MySQL-2 | 3.15.3 | Node.js |
| `mariadb` | libmariadb | 3.4.10 | the host's MariaDB 12.3 client (extra, not in the plan's list) |

**Not captured: go-sql-driver.** It was neither installed nor cached on the host. Fetching Go modules was outside the task's allowed downloads (crates.io and container images). To add it, run `capture.sh` with a go-sql-driver client.
