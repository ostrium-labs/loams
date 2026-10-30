#!/usr/bin/env bash
# Tests the bundle half of deploy/byoc/lib/loams-byoc-bootstrap against a local,
# test-signed release: a good bundle is unpacked and configured; a tampered
# bundle, a bad signature and a foreign signer are refused; the previous release
# key (rotated 2026-10) is still accepted, after the current one; a re-run keeps .env;
# and the secrets fetcher then fills .env from a mock secret store.
# (LOAMS_BYOC_SKIP_SYSTEM=1 skips Docker, systemd and the root check.)
# shellcheck disable=SC2015,SC2154 # A && B || C is used on purpose: C reports a failed check
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
boot="$repo/deploy/byoc/lib/loams-byoc-bootstrap"
tmp="$(mktemp -d "${TMPDIR:-/var/tmp}/byoc-boot.XXXXXX")"
pids=()
trap 'for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$tmp"' EXIT
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }

command -v gpg >/dev/null || { echo "skip bootstrap tests (gpg is not installed)"; exit 0; }

# a test signing key, and a second one for the foreign signer
export GNUPGHOME="$tmp/gnupg"; mkdir -m 700 "$GNUPGHOME"
gpg --batch --quiet --passphrase '' --quick-generate-key "Loams Test <test@example.invalid>" ed25519 sign never 2>/dev/null
gpg --batch --quiet --passphrase '' --quick-generate-key "Other <other@example.invalid>" ed25519 sign never 2>/dev/null
gpg --batch --quiet --passphrase '' --quick-generate-key "Loams Prev <prev@example.invalid>" ed25519 sign never 2>/dev/null
fpr="$(gpg --batch --with-colons --list-keys test@example.invalid | awk -F: '$1=="fpr"{print $10; exit}')"
other="$(gpg --batch --with-colons --list-keys other@example.invalid | awk -F: '$1=="fpr"{print $10; exit}')"
prev="$(gpg --batch --with-colons --list-keys prev@example.invalid | awk -F: '$1=="fpr"{print $10; exit}')"
# the published key file holds the current and the previous key, as docs/release/loams-release-key.asc does
gpg --batch --quiet --armor --export test@example.invalid prev@example.invalid >"$tmp/test-key.asc"

# a fake release: the repo's compose directory as the bundle, version 9.9.9
mkrelease() {  # dir
  local d="$1" v=9.9.9 name=loams-compose-9.9.9
  mkdir -p "$d/v$v" "$tmp/stage/$name"
  rm -rf "$tmp/stage/$name"; mkdir -p "$tmp/stage/$name"
  cp -a "$repo/deploy/release" "$repo/deploy/loams-postgres-dev" "$tmp/stage/$name/"
  tar -C "$tmp/stage" -czf "$d/v$v/$name.tar.gz" "$name"
  (cd "$d/v$v" && sha256sum "$name.tar.gz" >SHA256SUMS)
}
sign() {  # dir signer
  rm -f "$1/v9.9.9/SHA256SUMS.asc"
  gpg --batch --quiet --armor --detach-sign -u "$2" -o "$1/v9.9.9/SHA256SUMS.asc" "$1/v9.9.9/SHA256SUMS"
}
mkrelease "$tmp/rel"; sign "$tmp/rel" "$fpr"
python3 -I "$here/serve_dir.py" "$tmp/rel" "$tmp/http.port" &
pids+=("$!")
for _ in $(seq 50); do [[ -s "$tmp/http.port" ]] && break; sleep 0.1; done
port="$(cat "$tmp/http.port" 2>/dev/null || true)"
[[ -n "$port" ]] || { echo "FAIL the local release server did not start"; exit 1; }

cfg="$tmp/config.env"
cat >"$cfg" <<CFG
LOAMS_BYOC_CLOUD=aws
LOAMS_VERSION=9.9.9
LOAMS_BUCKET_URL=s3://my-bkt/loams
REGION=us-east-1
SECRET_PREFIX=loams/t
LOAMS_PROFILES=
TS_HOSTNAME=loams
DATA_DEVICE=
CFG
run() {  # root-dir [env...]
  local root="$1"; shift
  env LOAMS_BYOC_SKIP_SYSTEM=1 LOAMS_BYOC_CONFIG="$cfg" LOAMS_BYOC_ROOT="$root" LOAMS_BYOC_KEY="$tmp/test-key.asc" \
    LOAMS_BYOC_TEST_KEY_FPR="$fpr $prev" LOAMS_BYOC_RELEASE_URL="http://127.0.0.1:$port" "$@" "$boot"
}

# 1. a good, signed bundle
if run "$tmp/ok" >"$tmp/out1" 2>&1; then
  s="$tmp/ok/release"
  [[ -f "$s/compose.yaml" && -f "$tmp/ok/loams-postgres-dev/compose.yaml" ]] && ok "bundle unpacked as release/ and loams-postgres-dev/" || bad "bundle layout"
  grep -q '^LOAMS_VERSION=9.9.9$' "$s/.env" && grep -q '^LOAMS_BUCKET=s3://my-bkt/loams$' "$s/.env" && ok ".env has the version and bucket" || bad ".env: $(head -5 "$s/.env")"
  grep -q '^AWS_REGION=us-east-1$' "$s/secrets/engine.env" && ok "engine.env names the region" || bad "engine.env"
  ! grep -hvE '^[[:space:]]*#' "$s/.env" "$s/secrets/engine.env" | grep -qE 'AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY' && ok "no AWS key written" || bad "a key was written"
  [[ "$(stat -c %a "$s/.env")" == 600 && "$(stat -c %a "$s/secrets/engine.env")" == 600 ]] && ok ".env and engine.env are mode 600" || bad "modes"
  [[ "$(cat "$tmp/ok/.bundle-version")" == 9.9.9 ]] && ok "version marker written" || bad "marker"
else
  bad "a good signed bundle was refused: $(cat "$tmp/out1")"
fi

# 2. the fetcher then fills the secrets (a mock secret store)
cat >"$tmp/secrets.json" <<JSON
{"aws": {"loams/t/tailnet-authkey": "authkey-PLANTED-1", "loams/t/setup-token": "setup-PLANTED-2"}, "gcp": {}, "azure": {}}
JSON
python3 -I "$here/mock_cloud.py" "$tmp/port" "$tmp/secrets.json" &
pids+=("$!")
for _ in $(seq 50); do [[ -s "$tmp/port" ]] && break; sleep 0.1; done
mp="$(cat "$tmp/port")"
if env LOAMS_BYOC_CONFIG="$cfg" LOAMS_BYOC_STACK="$tmp/ok/release" LOAMS_BYOC_IMDS_URL="http://127.0.0.1:$mp/aws" \
     LOAMS_BYOC_SECRETS_URL="http://127.0.0.1:$mp/aws-sm" "$repo/deploy/byoc/lib/loams-byoc-secrets" aws >"$tmp/out2" 2>&1; then
  grep -q '^TS_AUTHKEY=authkey-PLANTED-1$' "$tmp/ok/release/.env" && grep -q '^LOAMS_BUCKET=s3://my-bkt/loams$' "$tmp/ok/release/.env" \
    && ok "bootstrap then fetch: .env has the bucket and the secrets" || bad "fetch after bootstrap"
else bad "fetch after bootstrap failed: $(cat "$tmp/out2")"; fi

# 3. a re-run keeps what the fetcher wrote
if run "$tmp/ok" >"$tmp/out3" 2>&1; then
  grep -q '^TS_AUTHKEY=authkey-PLANTED-1$' "$tmp/ok/release/.env" && ok "a re-run keeps the fetched secrets" || bad "re-run lost TS_AUTHKEY"
else bad "re-run failed: $(cat "$tmp/out3")"; fi

# 4. a tampered bundle is refused (its checksum no longer matches)
mkrelease "$tmp/rel"; sign "$tmp/rel" "$fpr"
printf 'x' >>"$tmp/rel/v9.9.9/loams-compose-9.9.9.tar.gz"
if run "$tmp/tamper" >"$tmp/out4" 2>&1; then bad "a tampered bundle was accepted"; else
  grep -q 'does not match SHA256SUMS' "$tmp/out4" && [[ ! -e "$tmp/tamper/release" ]] && ok "a tampered bundle is refused and nothing is unpacked" || bad "tamper: $(cat "$tmp/out4")"; fi

# 5. a SHA256SUMS signed by a foreign key is refused
mkrelease "$tmp/rel"; sign "$tmp/rel" "$other"
if run "$tmp/foreign" >"$tmp/out5" 2>&1; then bad "a foreign signature was accepted"; else
  grep -q 'is not signed by a Loams release key' "$tmp/out5" && ok "a foreign signer is refused" || bad "foreign: $(cat "$tmp/out5")"; fi

# 5b. a SHA256SUMS signed by the previous release key is still accepted, and says so
mkrelease "$tmp/rel"; sign "$tmp/rel" "$prev"
if run "$tmp/prev" >"$tmp/out5b" 2>&1; then
  grep -q "signed by the previous Loams release key $prev" "$tmp/out5b" && ok "the previous release key is accepted and named" || bad "prev: $(cat "$tmp/out5b")"
else bad "a bundle signed by the previous key was refused: $(cat "$tmp/out5b")"; fi
! grep -q 'previous Loams release key' "$tmp/out1" && ok "the current key is not reported as the previous one" || bad "current key named as previous"

# 6. a SHA256SUMS edited after signing is refused
mkrelease "$tmp/rel"; sign "$tmp/rel" "$fpr"
printf '# edited\n' >>"$tmp/rel/v9.9.9/SHA256SUMS"
if run "$tmp/edited" >"$tmp/out6" 2>&1; then bad "an edited SHA256SUMS was accepted"; else
  grep -q 'is not signed by a Loams release key' "$tmp/out6" && ok "an edited SHA256SUMS is refused" || bad "edited: $(cat "$tmp/out6")"; fi

# 7. latest and other non-versions are refused before any download
sed -i 's/^LOAMS_VERSION=.*/LOAMS_VERSION=latest/' "$cfg"
if run "$tmp/latest" >"$tmp/out7" 2>&1; then bad "latest was accepted"; else
  grep -q 'must be a release version' "$tmp/out7" && ok "latest is refused" || bad "latest: $(cat "$tmp/out7")"; fi

exit "$fail"
