#!/bin/sh
# openfga-init (the `authz` profile, design 76): makes OpenFGA ready for the engine, then
# exits.
#
# 1. Finds the store named `loams` (LOAMS_AUTHZ_OPENFGA_STORE_NAME), or creates it.
# 2. Writes Loams' model v1 (/etc/loams/openfga-model.json, generated from deploy/openfga by
#    deploy/openfga/model-json.sh) when its SHA-256 differs from the one written last, so a
#    restart adds no model version and an upgrade with a new model adds one.
# 3. Records the store id, the model id and the model's SHA-256 in
#    openfga.env in the `authz` volume (LOAMS_AUTHZ_DIR), which the engine reads through
#    config/stores.sh when LOAMS_AUTHZ=openfga.
#
# It writes no tuples: those come from the sync bridge (plan IA1, IA3). Every call carries
# OpenFGA's preshared key, passed to curl on stdin so it never appears in a process list.
set -eu

api="${OPENFGA_HTTP:-http://openfga:8080}"
name="${LOAMS_AUTHZ_OPENFGA_STORE_NAME:-loams}"
model=/etc/loams/openfga-model.json
dir="${LOAMS_AUTHZ_DIR:-/var/lib/loams-authz}"
out="$dir/openfga.env"

if [ -z "${LOAMS_AUTHZ_OPENFGA_KEY:-}" ]; then
  echo "openfga-init: the authz profile needs LOAMS_AUTHZ_OPENFGA_KEY in .env (openssl rand -hex 32)" >&2
  exit 78
fi

# call METHOD PATH [curl args...]: the response body on stdout; fails on any HTTP error.
call() {
  method="$1"
  path="$2"
  shift 2
  printf 'header = "Authorization: Bearer %s"\n' "$LOAMS_AUTHZ_OPENFGA_KEY" \
    | curl --config - -fsS --retry 10 --retry-connrefused --retry-delay 1 \
        -X "$method" -H 'Content-Type: application/json' "$@" "$api$path"
}

# The first value of JSON string field $1 in stdin (OpenFGA's ids are ULIDs: no escapes).
field() {
  grep -o "\"$1\":\"[^\"]*\"" | head -n 1 | cut -d '"' -f 4
}

# The recorded value of $1 in $out.
recorded() {
  [ -f "$out" ] && sed -n "s/^$1=\([0-9A-Za-z]*\)\$/\1/p" "$out" | head -n 1
}

sha="$(sha256sum "$model" | cut -d ' ' -f 1)"
store="$(recorded LOAMS_AUTHZ_OPENFGA_STORE || true)"
model_id="$(recorded LOAMS_AUTHZ_OPENFGA_MODEL || true)"

# A recorded store that OpenFGA no longer has (a new openfga-data volume) is found again.
if [ -n "$store" ] && ! call GET "/stores/$store" >/dev/null 2>&1; then
  store=
  model_id=
fi
if [ -n "$store" ] && [ -n "$model_id" ] \
  && [ "$(recorded LOAMS_AUTHZ_OPENFGA_MODEL_SHA256 || true)" = "$sha" ]; then
  echo "openfga-init: store $store already has this model ($model_id)"
  exit 0
fi

if [ -z "$store" ]; then
  store="$(call GET "/stores?name=$name&page_size=1" | field id || true)"
fi
if [ -z "$store" ]; then
  store="$(call POST /stores --data-binary "{\"name\":\"$name\"}" | field id)"
  echo "openfga-init: created store $name ($store)"
fi
model_id="$(call POST "/stores/$store/authorization-models" --data-binary "@$model" \
  | field authorization_model_id)"
if [ -z "$store" ] || [ -z "$model_id" ]; then
  echo "openfga-init: OpenFGA answered without a store or model id" >&2
  exit 1
fi

umask 022
mkdir -p "$dir"
printf 'LOAMS_AUTHZ_OPENFGA_STORE=%s\nLOAMS_AUTHZ_OPENFGA_MODEL=%s\nLOAMS_AUTHZ_OPENFGA_MODEL_SHA256=%s\n' \
  "$store" "$model_id" "$sha" > "$out.tmp"
mv "$out.tmp" "$out"
echo "openfga-init: store $store, model $model_id"
