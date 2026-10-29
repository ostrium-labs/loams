#!/usr/bin/env bash
# Fills the @@placeholders@@ of deploy/byoc/<cloud>/cloud-init.yaml and writes
# the result to stdout. Values are checked against strict patterns, because they
# end up in a root-run script. No value is a secret: the tailnet key and the
# setup token live in your cloud's secret store.
#
#   render.sh aws   --bucket B --region R   [common options] > user-data.yaml
#   render.sh gcp   --bucket B --project P  [common options] > user-data.yaml
#   render.sh azure --container C --storage-account A --vault V [--client-id ID] > user-data.yaml
#
# Common options:
#   --version V         Loams release [default: 0.1.3]
#   --install NAME      names the secrets [default: loams]; secrets are
#                       loams/<NAME>/... on AWS and loams-<NAME>-... elsewhere
#   --secret-prefix P   override the whole prefix
#   --profiles A,B      COMPOSE_PROFILES (tikv, rustfs, ...) [default: none: the
#                       light default; choose before the first start]
#   --hostname H        the engine's tailnet name [default: loams]
#   --data-device DEV   a disk to format ext4 and mount at /var/lib/docker
#   --confluent-integrations A,B   Confluent integrations whose keys the VM fetches
#                       (names only; the keys stay in the secret store) [default: none]
#   --gzip              gzip the output (AWS user-data is limited to 16 KB)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
die() { echo "render.sh: $*" >&2; exit 1; }

cloud="${1:-}"; [[ "$cloud" =~ ^(aws|gcp|azure)$ ]] || die "usage: render.sh aws|gcp|azure [options] (see the header)"
shift
declare -A v=([VERSION]=0.1.3 [INSTALL]=loams [PROFILES]="" [HOSTNAME]=loams [DATA_DEVICE]="" [CLIENT_ID]="" [CONFLUENT_INTEGRATIONS]="")
gzip_out=0
while (($#)); do
  case "$1" in
    --version) v[VERSION]="${2:?}"; shift 2 ;;
    --install) v[INSTALL]="${2:?}"; shift 2 ;;
    --secret-prefix) v[SECRET_PREFIX]="${2:?}"; shift 2 ;;
    --profiles) v[PROFILES]="${2-}"; shift 2 ;;
    --hostname) v[HOSTNAME]="${2:?}"; shift 2 ;;
    --data-device) v[DATA_DEVICE]="${2-}"; shift 2 ;;
    --confluent-integrations) v[CONFLUENT_INTEGRATIONS]="${2-}"; shift 2 ;;
    --bucket) v[BUCKET]="${2:?}"; shift 2 ;;
    --region) v[REGION]="${2:?}"; shift 2 ;;
    --project) v[PROJECT]="${2:?}"; shift 2 ;;
    --container) v[CONTAINER]="${2:?}"; shift 2 ;;
    --storage-account) v[STORAGE_ACCOUNT]="${2:?}"; shift 2 ;;
    --vault) v[VAULT]="${2:?}"; shift 2 ;;
    --client-id) v[CLIENT_ID]="${2-}"; shift 2 ;;
    --gzip) gzip_out=1; shift ;;
    -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

[[ -n "${v[SECRET_PREFIX]:-}" ]] || {
  [[ "${v[INSTALL]}" =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] || die "--install is a lowercase name (letters, digits, -)"
  case "$cloud" in aws) v[SECRET_PREFIX]="loams/${v[INSTALL]}" ;; *) v[SECRET_PREFIX]="loams-${v[INSTALL]}" ;; esac
}

need() { local k; for k in "$@"; do [[ -n "${v[$k]:-}" ]] || die "$cloud needs --${k,,}" ; done; }
check() {  # KEY REGEX WHAT
  [[ "${v[$1]:-}" =~ $2 ]] || die "$3 is not valid: '${v[$1]:-}'"
}
check VERSION '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$' "--version"
check SECRET_PREFIX '^[A-Za-z0-9][A-Za-z0-9/_-]{0,100}$' "--secret-prefix"
check PROFILES '^[a-z0-9,-]*$' "--profiles"
check HOSTNAME '^[a-z0-9][a-z0-9-]{0,62}$' "--hostname"
check DATA_DEVICE '^(/dev/[A-Za-z0-9/_.:-]+)?$' "--data-device"
check CONFLUENT_INTEGRATIONS '^([a-z0-9][a-z0-9-]{0,30}(,[a-z0-9][a-z0-9-]{0,30})*)?$' "--confluent-integrations"
case "$cloud" in
  aws)
    need BUCKET REGION
    check BUCKET '^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$' "--bucket"
    check REGION '^[a-z]{2}(-[a-z]+)+-[0-9]$' "--region" ;;
  gcp)
    need BUCKET PROJECT
    check BUCKET '^[a-z0-9][a-z0-9._-]{1,61}[a-z0-9]$' "--bucket"
    check PROJECT '^[a-z][a-z0-9-]{4,28}[a-z0-9]$' "--project" ;;
  azure)
    need CONTAINER STORAGE_ACCOUNT VAULT
    check CONTAINER '^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$' "--container"
    check STORAGE_ACCOUNT '^[a-z0-9]{3,24}$' "--storage-account"
    check VAULT '^[A-Za-z0-9][A-Za-z0-9-]{1,22}[A-Za-z0-9]$' "--vault"
    check CLIENT_ID '^([0-9a-fA-F-]{36})?$' "--client-id" ;;
esac

out="$(cat "$here/$cloud/cloud-init.yaml")"
for k in "${!v[@]}"; do out="${out//@@$k@@/${v[$k]}}"; done
# Keep the placeholders that appear only in comments out of the check.
if grep -v '^#' <<<"$out" | grep -qE '@@[A-Z_]+@@'; then
  die "unfilled placeholders: $(grep -v '^#' <<<"$out" | grep -oE '@@[A-Z_]+@@' | sort -u | tr '\n' ' ')"
fi
if ((gzip_out)); then printf '%s\n' "$out" | gzip -9n; else printf '%s\n' "$out"; fi
