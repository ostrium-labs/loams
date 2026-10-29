#!/usr/bin/env bash
# PgDog per Postgres major (NF2 Task 12, was NF1 Task 36; design §51 §12.4,
# §62 §4.4–§4.5). Runs the pinned, unmodified PgDog (PGDOG_IMAGE in
# conformance/compute/pins.env, by digest) in front of a Postgres of each major
# with podman, shaped as pg-control renders it for Loams (transaction pooling,
# SCRAM against a password_hash verifier, TLS on both hops with verify_ca, a
# `__ro` replica name with read_only, a session-mode role), and writes
# conformance/router/pgdog-pg<major>.tsv:
#
#   component <TAB> check <TAB> result <TAB> notes
#
# `result` compares PgDog with the same check run directly against the same
# server (the reference): `ok` (same outcome), `differs` (both succeed, the
# outcome differs), `error` (PgDog fails where the reference does not). The
# checker scripts/nf1/check_pgdog_majors.py gates the committed files in CI.
#
#   scripts/nf1/pgdog-majors.sh            # 17 and 18
#   scripts/nf1/pgdog-majors.sh 18         # one major
#   PGDOG_MAJORS_OUT=dir scripts/nf1/pgdog-majors.sh   # write elsewhere
#
# Local and manual only (podman). It removes only the containers and the
# network it created (named pgdog-majors-<major>-<pid>-*), never prunes.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out_dir=${PGDOG_MAJORS_OUT:-$repo/conformance/router}

PGDOG_IMAGE=$(sed -n 's/^PGDOG_IMAGE=//p' "$repo/conformance/compute/pins.env")
[ -n "$PGDOG_IMAGE" ] || { echo "pgdog-majors: no PGDOG_IMAGE in conformance/compute/pins.env" >&2; exit 1; }

# Vanilla Postgres images by digest: the tags as published on 2026-10-07
# (docker-library/repo-info 938ae675; the 2026-10-21 and 2026-10-28 rebuilds
# are not 14 days old). 18 runs against vanilla 18.6 until NF2 Task 8
# publishes loams-postgres-compute-v18; then it is re-run against that image.
declare -A PG_IMAGE=(
  [17]=docker.io/library/postgres:17.11@sha256:2d2b8998d31037bf721cfdf764d76ba74171b4fab3431b7f72c27c56ddbdf9e3
  [18]=docker.io/library/postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336
)
# libpq and psql 18 (max_protocol_version) for every major.
CLIENT_IMAGE=${PG_IMAGE[18]}

majors=("$@")
[ ${#majors[@]} -gt 0 ] || majors=(17 18)
for m in "${majors[@]}"; do
  [ -n "${PG_IMAGE[$m]:-}" ] || { echo "pgdog-majors: no image for major $m" >&2; exit 1; }
done
command -v podman >/dev/null || { echo "pgdog-majors: needs podman" >&2; exit 1; }
command -v openssl >/dev/null || { echo "pgdog-majors: needs openssl" >&2; exit 1; }

work=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/pgdog-majors.XXXXXX")
created_containers=()
created_networks=()
cleanup() {
  local c n
  for c in "${created_containers[@]}"; do podman rm -f -t 2 "$c" >/dev/null 2>&1 || true; done
  for n in "${created_networks[@]}"; do podman network rm "$n" >/dev/null 2>&1 || true; done
  rm -rf "$work"
}
trap cleanup EXIT

# --- fixtures -------------------------------------------------------------------

mkdir -p "$work/certs" "$work/pgdog"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=pgdog-majors-ca" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
  -keyout "$work/certs/ca.key" -out "$work/certs/ca.crt" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=pg" \
  -keyout "$work/certs/server.key" -out "$work/certs/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:pg,DNS:pgdog,DNS:localhost,IP:127.0.0.1\n' >"$work/certs/ext"
openssl x509 -req -in "$work/certs/server.csr" -CA "$work/certs/ca.crt" -CAkey "$work/certs/ca.key" \
  -CAcreateserial -days 2 -extfile "$work/certs/ext" -out "$work/certs/server.crt" 2>/dev/null
chmod 644 "$work/certs/"*

# A SCRAM-SHA-256 verifier, as pg-control writes `password_hash`.
verifier() {
  python3 -I -c '
import base64, hashlib, hmac, os, sys
pw = sys.argv[1].encode(); salt = os.urandom(16); it = 4096
s = hashlib.pbkdf2_hmac("sha256", pw, salt, it)
ck = hmac.new(s, b"Client Key", hashlib.sha256).digest()
sk = hmac.new(s, b"Server Key", hashlib.sha256).digest()
b = lambda x: base64.b64encode(x).decode()
print(f"SCRAM-SHA-256${it}:{b(salt)}${b(hashlib.sha256(ck).digest())}:{b(sk)}")' "$1"
}

cat >"$work/init.sql" <<'SQL'
CREATE ROLE app LOGIN PASSWORD 'backend-app';
CREATE ROLE migrator LOGIN PASSWORD 'backend-migrator';
CREATE DATABASE app OWNER app;
CREATE DATABASE ref OWNER app;
\c app
CREATE EXTENSION btree_gist;
GRANT CREATE ON SCHEMA public TO migrator;
\c ref
CREATE EXTENSION btree_gist;
GRANT CREATE ON SCHEMA public TO migrator;
SQL

cat >"$work/pgdog/pgdog.toml" <<'TOML'
# NF2 Task 12: the shape pg-control renders (crates/loams-pg-control/src/pgdog/render.rs).
[general]
host = "0.0.0.0"
port = 6432
auth_type = "scram"
pooler_mode = "transaction"
checkout_timeout = 20000
connect_timeout = 1000
connect_attempts = 1
connect_attempt_delay = 500
ban_timeout = 1000
healthcheck_interval = 30000
idle_healthcheck_interval = 0
min_pool_size = 0
idle_timeout = 30000
tls_certificate = "/etc/pgdog/tls/server.crt"
tls_private_key = "/etc/pgdog/tls/server.key"
tls_client_required = true
tls_verify = "verify_ca"
tls_server_ca_certificate = "/etc/pgdog/tls/ca.crt"

[admin]
name = "admin"
user = "admin"
password = "admin-secret"

[[databases]]
name = "app"
host = "pg"
port = 5432
database_name = "app"
role = "primary"
pool_size = 10
pooler_mode = "transaction"

[[databases]]
name = "app__ro"
host = "pg"
port = 5432
database_name = "app"
role = "replica"
read_only = true
pool_size = 10
pooler_mode = "transaction"
TOML

cat >"$work/pgdog/users.toml" <<TOML
[[users]]
name = "app"
databases = ["app", "app__ro"]
password_hash = "$(verifier client-app)"
server_user = "app"
server_password = "backend-app"

[[users]]
name = "migrator"
databases = ["app"]
password_hash = "$(verifier client-migrator)"
server_user = "migrator"
server_password = "backend-migrator"
pooler_mode = "session"
TOML
chmod 644 "$work/init.sql" "$work/pgdog/"*

# --- helpers ----------------------------------------------------------------------

rows=()
# row <component> <check> <result> <notes>: tabs, newlines and backslashes escaped.
esc() { local s=${1//\\/\\\\}; s=${s//$'\t'/\\t}; s=${s//$'\n'/\\n}; printf '%s' "${s:0:400}"; }
row() { rows+=("$(esc "$1")"$'\t'"$(esc "$2")"$'\t'"$3"$'\t'"$(esc "$4")"); echo "  [$3] $1: $2" >&2; }

TLS='sslmode=verify-ca sslrootcert=/certs/ca.crt'
dsn_ref() { echo "host=pg port=5432 dbname=ref user=$1 $TLS ${2:-}"; }
dsn_dog() { echo "host=pgdog port=6432 dbname=$2 user=$1 $TLS ${3:-}"; }
backend_pw() { echo "backend-$1"; }
client_pw() { echo "client-$1"; }

# psql in the client container; prints stdout+stderr, exit status in $?.
psql_run() { # <password> <dsn> <sql>
  podman exec -i -e PGPASSWORD="$1" -e PGCONNECT_TIMEOUT=10 "$client" \
    psql -X -q -A -t -v VERBOSITY=sqlstate -v ON_ERROR_STOP=0 "$2" <<<"$3" 2>&1 || true
}
norm() { sed -E 's/^psql:<stdin>:[0-9]+: //'; }

# sqlcheck <component> <check> <user> <routed db> <sql> [ref extra dsn]
sqlcheck() {
  local comp=$1 name=$2 user=$3 db=$4 sql=$5 refx=${6:-} ref dog
  ref=$(psql_run "$(backend_pw "$user")" "$(dsn_ref "$user" "$refx")" "$sql" | norm) || true
  dog=$(psql_run "$(client_pw "$user")" "$(dsn_dog "$user" "$db")" "$sql" | norm) || true
  if [ "$ref" = "$dog" ]; then
    row "$comp" "$name" ok "same result as the server directly: $(head -c 160 <<<"$dog")"
  elif grep -qE '(ERROR|FATAL|error|failed)' <<<"$dog" && ! grep -qE '(ERROR|FATAL)' <<<"$ref"; then
    row "$comp" "$name" error "through PgDog: $dog | directly: $ref"
  else
    row "$comp" "$name" differs "through PgDog: $dog | directly: $ref"
  fi
}

# key_valid <asked> <negotiated> <key bytes>: the version is at most the asked
# one, and the key fits it (3.0: 4 bytes; 3.2: 4 to 256 bytes).
key_valid() {
  case $2 in 3.0) [ "$3" = 4 ] ;; 3.2) [ "$1" = 3.2 ] && [ "$3" -ge 4 ] && [ "$3" -le 256 ] ;; *) false ;; esac
}

probe() { # <port> <user> <password> <db> <protocol> <cancel>
  python3 -I "$here/pgdog_wire.py" --port "$1" --user "$2" --password "$3" --db "$4" \
    --protocol "$5" --tls-ca "$work/certs/ca.crt" --cancel "$6"
}
jget() { python3 -I -c 'import json,sys; v=json.loads(sys.argv[1]).get(sys.argv[2]); print("" if v is None else v)' "$1" "$2"; }

# psql run by SIGINT after 2 s, as an interactive Ctrl-C (libpq's cancel).
psql_sigint() { # <password> <dsn>
  podman exec -i -e PGPASSWORD="$1" "$client" sh -c '
    psql -X -q -A -t -v VERBOSITY=sqlstate -c "SELECT pg_sleep(20)" "$1" >/tmp/o 2>&1 & p=$!
    sleep 2; kill -INT $p; wait $p; cat /tmp/o' sh "$2" 2>&1 | norm || true
}

# --- one major -------------------------------------------------------------------

run_major() {
  local major=$1 image=${PG_IMAGE[$1]} tag="pgdog-majors-$1-$$"
  local net=$tag pg=$tag-pg dog=$tag-pgdog
  client=$tag-client
  rows=()
  echo "pgdog-majors: Postgres $major ($image) behind $PGDOG_IMAGE" >&2

  podman network create "$net" >/dev/null && created_networks+=("$net")
  podman run -d --name "$pg" --network "$net" --network-alias pg -p 127.0.0.1::5432 \
    -e POSTGRES_PASSWORD=super-secret \
    -v "$work/certs:/certs:ro" -v "$work/init.sql:/docker-entrypoint-initdb.d/init.sql:ro" \
    --entrypoint sh "$image" -c '
      mkdir -p /etc/pgtls && cp /certs/server.crt /certs/server.key /etc/pgtls/ &&
      chown -R postgres /etc/pgtls && chmod 600 /etc/pgtls/server.key &&
      exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/etc/pgtls/server.crt \
        -c ssl_key_file=/etc/pgtls/server.key -c password_encryption=scram-sha-256' >/dev/null
  created_containers+=("$pg")
  local i
  for i in $(seq 1 60); do
    podman exec "$pg" pg_isready -q -U postgres -h 127.0.0.1 2>/dev/null && break
    sleep 1
  done
  podman exec "$pg" pg_isready -q -U postgres -h 127.0.0.1 || { podman logs "$pg" >&2; return 1; }

  podman run -d --name "$dog" --network "$net" --network-alias pgdog -p 127.0.0.1::6432 \
    -v "$work/pgdog:/etc/pgdog:ro" -v "$work/certs:/etc/pgdog/tls:ro" \
    --cap-drop ALL --security-opt no-new-privileges:true \
    "$PGDOG_IMAGE" pgdog --config /etc/pgdog/pgdog.toml --users /etc/pgdog/users.toml >/dev/null
  created_containers+=("$dog")
  podman run -d --name "$client" --network "$net" -v "$work/certs:/certs:ro" \
    --entrypoint sleep "$CLIENT_IMAGE" infinity >/dev/null
  created_containers+=("$client")
  for i in $(seq 1 30); do
    psql_run client-app "$(dsn_dog app app)" 'SELECT 1' 2>/dev/null | grep -qx 1 && break
    sleep 1
  done

  local pg_port dog_port
  pg_port=$(podman port "$pg" 5432/tcp | head -1 | sed 's/.*://')
  dog_port=$(podman port "$dog" 6432/tcp | head -1 | sed 's/.*://')
  local server_version
  server_version=$(psql_run backend-app "$(dsn_ref app)" 'SHOW server_version')

  # protocol: libpq 18 connects at 3.0 and 3.2, and what the wire negotiates.
  local v out ref dog_out
  for v in 3.0 3.2; do
    ref=$(psql_run backend-app "$(dsn_ref app "max_protocol_version=$v")" 'SELECT current_user')
    dog_out=$(psql_run client-app "$(dsn_dog app app "max_protocol_version=$v")" 'SELECT current_user')
    if [ "$ref" = "$dog_out" ]; then
      row protocol "libpq 18 connects with max_protocol_version=$v" ok "psql 18: $dog_out"
    else
      row protocol "libpq 18 connects with max_protocol_version=$v" error "through PgDog: $dog_out | directly: $ref"
    fi
    ref=$(probe "$pg_port" app backend-app ref "$v" none)
    out=$(probe "$dog_port" app client-app app "$v" none)
    local rn dn rk dk
    rn=$(jget "$ref" negotiated); dn=$(jget "$out" negotiated)
    rk=$(jget "$ref" key_len); dk=$(jget "$out" key_len)
    if [ "$(jget "$out" connected)" != True ]; then
      row protocol "StartupMessage $v: negotiated version and cancel-key length" error "through PgDog: $(jget "$out" error) | directly: $rn, key $rk bytes"
    elif [ "$rn" = "$dn" ] && [ "$rk" = "$dk" ]; then
      row protocol "StartupMessage $v: negotiated version and cancel-key length" ok "both negotiate $dn (NegotiateProtocolVersion: $(jget "$out" negotiate_message)), key $dk bytes"
    elif key_valid "$v" "$dn" "$dk"; then
      # PgDog terminates the protocol: it may speak a version to the client
      # that the server behind it does not. Valid when the version is one the
      # client asked for or below and the key fits it; the cancel rows check
      # that the key works.
      row protocol "StartupMessage $v: negotiated version and cancel-key length" ok "PgDog speaks its own version to the client: $dn (NegotiateProtocolVersion: $(jget "$out" negotiate_message)), its own key of $dk bytes | the server directly: $rn (NegotiateProtocolVersion: $(jget "$ref" negotiate_message)), key $rk bytes"
    else
      row protocol "StartupMessage $v: negotiated version and cancel-key length" differs "through PgDog: $dn (NegotiateProtocolVersion: $(jget "$out" negotiate_message)), key $dk bytes | directly: $rn (NegotiateProtocolVersion: $(jget "$ref" negotiate_message)), key $rk bytes"
    fi
  done

  # cancel: a CancelRequest with the key the connection got, over TLS and plaintext.
  local mode
  for v in 3.0 3.2; do
    for mode in tls plain; do
      ref=$(probe "$pg_port" app backend-app ref "$v" "$mode")
      out=$(probe "$dog_port" app client-app app "$v" "$mode")
      local rc dc
      rc=$(jget "$ref" cancel); dc=$(jget "$out" cancel)
      local what="CancelRequest ($mode) with the protocol $v key ($(jget "$out" key_len) bytes through PgDog) stops pg_sleep"
      if [ -z "$rc" ] || [ -z "$dc" ]; then row cancel "$what" error "no result; through PgDog: $(jget "$out" error) | directly: $(jget "$ref" error)"
      elif [ "$rc" = "$dc" ]; then row cancel "$what" ok "both: $dc"
      elif [ "$rc" = 57014 ]; then row cancel "$what" error "through PgDog: ${dc:-$(jget "$out" error)} | directly: $rc"
      else row cancel "$what" differs "through PgDog: $dc | directly: $rc"; fi
    done
    ref=$(psql_sigint backend-app "$(dsn_ref app "max_protocol_version=$v")")
    out=$(psql_sigint client-app "$(dsn_dog app app "max_protocol_version=$v")")
    if [ "$ref" = "$out" ]; then row cancel "psql 18 Ctrl-C (libpq cancel) at max_protocol_version=$v" ok "both: $out"
    else row cancel "psql 18 Ctrl-C (libpq cancel) at max_protocol_version=$v" error "through PgDog: $out | directly: $ref"; fi
  done

  # auth: SCRAM against the password_hash verifier; wrong password; channel binding.
  out=$(psql_run client-app "$(dsn_dog app app)" 'SELECT current_user')
  [ "$out" = app ] && row auth "SCRAM-SHA-256 login against a password_hash verifier" ok "current_user = app" \
    || row auth "SCRAM-SHA-256 login against a password_hash verifier" error "$out"
  out=$(psql_run wrong "$(dsn_dog app app)" 'SELECT 1')
  ref=$(psql_run wrong "$(dsn_ref app)" 'SELECT 1')
  if grep -q '^1$' <<<"$out"; then row auth "a wrong password is refused" error "accepted: $out"
  else row auth "a wrong password is refused" ok "refused; through PgDog: $(tail -1 <<<"$out") | directly: $(tail -1 <<<"$ref")"; fi
  ref=$(psql_run backend-app "$(dsn_ref app channel_binding=require)" 'SELECT current_user')
  out=$(psql_run client-app "$(dsn_dog app app channel_binding=require)" 'SELECT current_user')
  if [ "$ref" = "$out" ]; then row auth "SCRAM-SHA-256-PLUS (channel_binding=require)" ok "both: $out"
  else row auth "SCRAM-SHA-256-PLUS (channel_binding=require)" error "through PgDog: $out | directly: $ref"; fi
  out=$(psql_run client-migrator "$(dsn_dog migrator app)" 'SELECT current_user')
  [ "$out" = migrator ] && row auth "backend login as server_user with server_password (SCRAM)" ok "current_user = migrator" \
    || row auth "backend login as server_user with server_password (SCRAM)" error "$out"

  # tls: the client hop is required, the server hop is verified.
  out=$(podman exec -i -e PGPASSWORD=client-app "$client" psql -X -q -A -t \
    "host=pgdog port=6432 dbname=app user=app sslmode=disable" -c 'SELECT 1' 2>&1 || true)
  if grep -q '^1$' <<<"$out"; then row tls "a plaintext client is refused (tls_client_required)" error "accepted"
  else row tls "a plaintext client is refused (tls_client_required)" ok "refused: $(tail -1 <<<"$out")"; fi
  out=$(psql_run client-app "$(dsn_dog app app)" 'SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()')
  [ "$out" = t ] && row tls "the server hop is TLS (tls_verify = verify_ca)" ok "pg_stat_ssl.ssl = t" \
    || row tls "the server hop is TLS (tls_verify = verify_ca)" error "$out"

  # query: the 17 baseline through transaction pooling (`app`).
  sqlcheck query "simple query" app app "SELECT 1 + 1, 'x'::text"
  sqlcheck query "extended protocol with parameters (\\bind)" app app "SELECT \$1::int * 2 \\bind 21 \\g"
  sqlcheck query "named prepared statement (\\parse, \\bind_named)" app app \
    "SELECT \$1::text || '!' \\parse s1
\\bind_named s1 hi \\g"
  sqlcheck query "transaction with SET LOCAL" app app \
    "BEGIN; SET LOCAL statement_timeout = '5s'; SHOW statement_timeout; COMMIT;"
  sqlcheck query "CREATE TABLE, INSERT … RETURNING, UPDATE in a CTE" app app \
    "CREATE TABLE b17 (id int PRIMARY KEY, v int); INSERT INTO b17 VALUES (1, 10), (2, 20) RETURNING id, v;
WITH u AS (UPDATE b17 SET v = v + 1 WHERE id = 1 RETURNING v) SELECT v FROM u;"
  sqlcheck query "stored generated column" app app \
    "CREATE TABLE g17 (a int, b int GENERATED ALWAYS AS (a * 2) STORED); INSERT INTO g17 (a) VALUES (3); SELECT a, b FROM g17;"
  sqlcheck query "MERGE … RETURNING merge_action() (17)" app app \
    "MERGE INTO b17 t USING (VALUES (2, 5), (3, 7)) s(id, v) ON t.id = s.id
WHEN MATCHED THEN UPDATE SET v = s.v WHEN NOT MATCHED THEN INSERT VALUES (s.id, s.v)
RETURNING merge_action(), t.id, t.v;"
  sqlcheck query "JSON_TABLE (17)" app app \
    "SELECT * FROM JSON_TABLE('[{\"a\":1},{\"a\":2}]'::jsonb, '\$[*]' COLUMNS (a int PATH '\$.a'));"
  sqlcheck query "COPY FROM STDIN and COPY TO STDOUT" app app \
    "CREATE TABLE c17 (id int, t text);
COPY c17 FROM STDIN;
1	one
2	two
\\.
COPY c17 TO STDOUT;"
  sqlcheck query "SELECT … FOR UPDATE in a transaction" app app \
    "BEGIN; SELECT v FROM b17 WHERE id = 1 FOR UPDATE; COMMIT;"
  sqlcheck query "a unique violation returns its SQLSTATE" app app "INSERT INTO b17 VALUES (1, 0);"

  if [ "$major" -ge 18 ]; then
    # query: the 18 syntax PgDog's parser sees (§62 §4.5).
    sqlcheck query "virtual generated column (VIRTUAL)" app app \
      "CREATE TABLE vg (a int, b int GENERATED ALWAYS AS (a * 2) VIRTUAL); INSERT INTO vg (a) VALUES (4); SELECT a, b FROM vg;"
    sqlcheck query "generated column of the default kind (virtual at 18)" app app \
      "CREATE TABLE vd (a int, b int GENERATED ALWAYS AS (a + 1)); INSERT INTO vd (a) VALUES (1); SELECT a, b, attgenerated FROM vd, pg_attribute WHERE attrelid = 'vd'::regclass AND attname = 'b';"
    sqlcheck query "UPDATE … RETURNING OLD.v, NEW.v" app app \
      "CREATE TABLE r18 (id int PRIMARY KEY, v int); INSERT INTO r18 VALUES (1, 1), (2, 2);
UPDATE r18 SET v = v * 10 WHERE id = 1 RETURNING old.v, new.v;"
    sqlcheck query "UPDATE … RETURNING WITH (OLD AS o, NEW AS n)" app app \
      "UPDATE r18 SET v = v + 1 WHERE id = 2 RETURNING WITH (OLD AS o, NEW AS n) o.v, n.v;"
    sqlcheck query "INSERT … ON CONFLICT DO UPDATE RETURNING old.*, new.*" app app \
      "INSERT INTO r18 VALUES (1, 0) ON CONFLICT (id) DO UPDATE SET v = excluded.v RETURNING old.*, new.*;"
    sqlcheck query "DELETE … RETURNING OLD.*" app app "DELETE FROM r18 WHERE id = 2 RETURNING old.*;"
    sqlcheck query "MERGE … RETURNING merge_action(), old, new" app app \
      "MERGE INTO r18 t USING (VALUES (1, 9), (5, 5)) s(id, v) ON t.id = s.id
WHEN MATCHED THEN UPDATE SET v = s.v WHEN NOT MATCHED THEN INSERT VALUES (s.id, s.v)
RETURNING merge_action(), old.v, new.v;"
    sqlcheck query "RETURNING OLD/NEW over the extended protocol (\\bind)" app app \
      "UPDATE r18 SET v = \$1 WHERE id = 1 RETURNING old.v, new.v \\bind 77 \\g"
    sqlcheck query "PRIMARY KEY (id, valid WITHOUT OVERLAPS)" app app \
      "CREATE TABLE p18 (id int, valid daterange, PRIMARY KEY (id, valid WITHOUT OVERLAPS));
INSERT INTO p18 VALUES (1, '[2026-01-01,2026-02-01)'), (1, '[2026-02-01,2026-03-01)');
INSERT INTO p18 VALUES (1, '[2026-01-15,2026-01-20)');"
    sqlcheck query "UNIQUE (id, valid WITHOUT OVERLAPS)" app app \
      "CREATE TABLE u18 (id int, valid tstzrange, UNIQUE (id, valid WITHOUT OVERLAPS)); INSERT INTO u18 VALUES (1, '[2026-01-01,2026-01-02)'); SELECT count(*) FROM u18;"
    sqlcheck query "FOREIGN KEY (id, PERIOD valid) REFERENCES … (id, PERIOD valid)" app app \
      "CREATE TABLE f18 (id int, valid daterange, FOREIGN KEY (id, PERIOD valid) REFERENCES p18 (id, PERIOD valid));
INSERT INTO f18 VALUES (1, '[2026-01-10,2026-02-10)');
INSERT INTO f18 VALUES (1, '[2026-02-10,2026-04-01)');"
    sqlcheck query "uuidv7() (18)" app app "SELECT uuidv7() IS NOT NULL, uuid_extract_version(uuidv7());"
  fi

  # session: a session-mode role (pg-control's migrator) keeps session state.
  local sess="SET application_name = 'nf2'; SHOW application_name; CREATE TEMP TABLE t (a int); INSERT INTO t VALUES (1); SELECT count(*) FROM t;"
  [ "$major" -ge 18 ] && sess+=" CREATE TABLE ms18 (id int PRIMARY KEY, v int GENERATED ALWAYS AS (id * 3) VIRTUAL); INSERT INTO ms18 (id) VALUES (1) RETURNING old.v, new.v;"
  sqlcheck session "session pooling keeps SET and temporary tables$([ "$major" -ge 18 ] && echo '; 18 syntax')" migrator app "$sess"

  # read-only: the `__ro` name (role replica, read_only), compared with a
  # read-only session directly.
  local ro='default_transaction_read_only=on'
  sqlcheck read-only "a read on the __ro name" app app__ro "SELECT id, v FROM b17 ORDER BY id;" "options=-c$ro"
  sqlcheck read-only "a write on the __ro name is refused" app app__ro "INSERT INTO b17 VALUES (9, 9);" "options=-c$ro"
  if [ "$major" -ge 18 ]; then
    sqlcheck read-only "a read of a virtual generated column on the __ro name" app app__ro "SELECT a, b FROM vg;" "options=-c$ro"
    sqlcheck read-only "UPDATE … RETURNING OLD/NEW on the __ro name is refused" app app__ro \
      "UPDATE r18 SET v = 0 WHERE id = 1 RETURNING old.v, new.v;" "options=-c$ro"
  fi

  local file="$out_dir/pgdog-pg$major.tsv"
  {
    printf 'component\tcheck\tresult\tnotes\n'
    printf '%s\n' "${rows[@]}"
  } >"$file"
  echo "pgdog-majors: wrote $file (server $server_version)" >&2

  podman rm -f -t 0 "$client" "$dog" "$pg" >/dev/null
  podman network rm "$net" >/dev/null
}

for m in "${majors[@]}"; do run_major "$m"; done
