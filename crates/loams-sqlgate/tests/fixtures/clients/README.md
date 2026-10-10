# Captured client handshakes

These are real MySQL client handshakes against keyspace-mode TiDB v8.5.8 (`pingcap/tidb@sha256:df168c76…`, the spike stack). They were captured on 2026-10-09 with `scripts/sqlgate/capture/capture.sh`, through its recording proxy (`proxy.py`).

- **Pinned clients.** Every client is pinned. The script refuses an image that is not pinned by digest, and refuses a client whose version differs.
- **Format.** Each `*.jsonl` file holds one MySQL packet payload per line, with `dir` (`s2c` or `c2s`), `seq` and `hex`.
- **The user.** Every capture logs in as `loams_cap` with the test password `capture`, created `IDENTIFIED WITH caching_sha2_password`. The password is a fixed test value, not a secret.

**`<client>.jsonl`, the plain run:**
1. TiDB's greeting (plugin `mysql_native_password`).
2. The client's `HandshakeResponse41` (a native scramble).
3. TiDB's `AuthSwitchRequest` to `caching_sha2_password`.
4. The client's SHA-2 scramble, which `tests/it/clients.rs` reproduces.
5. `AuthMoreData` asking for full authentication.
6. The client's request for the RSA key (`0x02`).
7. ERR 1045 (plaintext, and TiDB has no RSA key).

**`<client>-ssl.jsonl`.** The proxy rewrites the greeting to advertise `CLIENT_SSL` and records the client's next packet:
- `mysql84` and `connector-j` answer with an `SSLRequest`;
- `mysql2`, `go-sql-driver` (no TLS option) and `mariadb` (`--skip-ssl`) answer with a plaintext response.

**`<client>-tls-sha2.jsonl`, mysql 8.4 and Connector/J.** The proxy's greeting offers `CLIENT_SSL` and `caching_sha2_password`. The proxy terminates the client's TLS with a throwaway certificate and records the decrypted packets. It relays them to TiDB in plaintext, with `CLIENT_SSL` cleared and sequence ids shifted. The capture holds:
- the `SSLRequest`;
- the response, whose SHA-2 scramble is of the greeting nonce;
- TiDB's switch and the scramble again;
- `AuthMoreData` asking for full authentication;
- the client's cleartext password `capture\0` (over TLS);
- OK, then the first query.

`tls_sha2_captures_drive_the_connection_phase` replays these client bytes through `ConnectionPhase`.

| File | Client | Version | Pinned by |
|---|---|---|---|
| `mysql84` | `mysql` (libmysql) | 8.4.10 | `docker.io/library/mysql@sha256:02aa6476f6b675e5d4d19c5b437798b1b8a4048ae39383eac609814998ed15b8`; the script checks `mysql --version` |
| `connector-j` | MySQL Connector/J | 9.7.0 | `mysql-connector-j-9.7.0.jar` (`CONNECTOR_J`; the script checks `Implementation-Version`), OpenJDK 25 |
| `mysql2` | Node-MySQL-2 | 3.15.3 | `scripts/sqlgate/capture/node/{package.json,package-lock.json}` (resolved `--before=2026-09-25`), `npm ci` inside `docker.io/library/node@sha256:c385ec44d77c785e2364ac0c9b150809a0fdc17fde3dbf061e3dad07242c6a85` (Node 22.20.0, 2025-09-24) |
| `go-sql-driver` | Go-MySQL-Driver | v1.9.3 (2025-06-13) | `scripts/sqlgate/capture/go/{go.mod,go.sum}`, `go run` inside `docker.io/library/golang@sha256:ebd54034f076819b3054f155db53660ded951612bb4dfd277f933e62059e5d5a` (Go 1.25.0, 2025-08-21) |
| `mariadb` | libmariadb | 3.4.10 | the host's MariaDB 12.3.3 client (the script checks `mariadb --version`); an extra, not in the plan's list |

**What the clients send and where their modules come from.**
- go-sql-driver sends no `_client_version` attribute.
- The Node and Go modules are downloaded only inside their containers; nothing is installed on the host.
