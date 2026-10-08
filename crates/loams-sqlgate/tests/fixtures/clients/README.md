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
- `mysql2` (no `ssl` option), `go-sql-driver` (no `tls` option) and `mariadb` (`--skip-ssl`) answer with a plaintext response.

| File | Client | Version | Source |
|---|---|---|---|
| `mysql84` | `mysql` (libmysql) | 8.4.10 | `docker.io/library/mysql@sha256:02aa6476f6b675e5d4d19c5b437798b1b8a4048ae39383eac609814998ed15b8` |
| `connector-j` | MySQL Connector/J | 9.7.0 | `mysql-connector-j-9.7.0.jar`, OpenJDK 25 |
| `mysql2` | Node-MySQL-2 | 3.15.3 | Node.js |
| `go-sql-driver` | Go-MySQL-Driver | v1.9.3 (2025-06-13) | `go run` inside `docker.io/library/golang@sha256:ebd54034f076819b3054f155db53660ded951612bb4dfd277f933e62059e5d5a` (Go 1.25.0, 2025-08-21); `scripts/sqlgate/capture/go/{go.mod,go.sum}` pin the module. It was captured 2026-10-09 with `CLIENTS=go-sql-driver GOLANG_IMAGE=… capture.sh` |
| `mariadb` | libmariadb | 3.4.10 | the host's MariaDB 12.3 client (extra, not in the plan's list) |

**go-sql-driver** sends no `_client_version` attribute, and does not use TLS unless asked, so its `-ssl` capture is a plaintext response. Its Go module is downloaded only inside the container; nothing is installed on the host.
