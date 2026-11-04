#!/usr/bin/env bash
# Builds deploy/byoc/<cloud>/cloud-init.yaml from cloud-init.tmpl.yaml, lib/*
# and <cloud>/config.env, and <cloud>/cloud-init-k3s.yaml (the k3s home) from
# cloud-init-k3s.tmpl.yaml, lib/*, k3s.env and k3s/, so the scripts a VM runs are
# the files in lib/ (which the linters and the tests cover) and the cloud-init
# files cannot drift.
#
#   gen-cloud-init.sh            rewrite the six files
#   gen-cloud-init.sh --check    fail if any file is out of date (CI)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
check=0; [[ "${1:-}" == "--check" ]] && check=1

indent() { sed 's/^\(.\)/      \1/' "$1"; }

build() {  # $1 = cloud, $2 = display name, $3 = template file name
  local cloud="$1" name="$2" tmpl="$3" line
  while IFS= read -r line || [[ -n "$line" ]]; do
    case "$line" in
      @@CONFIG@@)    indent "$here/$cloud/config.env" ;;
      @@KEY@@)       indent "$repo/docs/release/loams-release-key.asc" ;;
      @@BOOTSTRAP@@) indent "$here/lib/loams-byoc-bootstrap" ;;
      @@SECRETS@@)   indent "$here/lib/loams-byoc-secrets" ;;
      @@UNIT@@)      indent "$here/lib/loams-byoc.service" ;;
      @@CONFLUENT_UNIT@@)  indent "$here/lib/loams-byoc-confluent.service" ;;
      @@CONFLUENT_TIMER@@) indent "$here/lib/loams-byoc-confluent.timer" ;;
      @@K3S_PINS@@)        indent "$here/k3s.env" ;;
      @@K3S_BOOTSTRAP@@)   indent "$here/lib/loams-k3s-bootstrap" ;;
      @@K3S_UNIT@@)        indent "$here/lib/loams-k3s.service" ;;
      @@K3S_IMDS_UNIT@@)   indent "$here/lib/loams-k3s-imds-block.service" ;;
      @@K3S_ACCESS_DB@@)   indent "$here/k3s/access-db.yaml.tmpl" ;;
      *) line="${line//@@CLOUD_NAME@@/$name}"; printf '%s\n' "${line//@@CLOUD@@/$cloud}" ;;
    esac
  done <"$here/$tmpl"
}

rc=0
for spec in "aws:AWS" "gcp:Google Cloud" "azure:Azure"; do
  cloud="${spec%%:*}"; name="${spec#*:}"
  for pair in "cloud-init.tmpl.yaml:cloud-init.yaml" "cloud-init-k3s.tmpl.yaml:cloud-init-k3s.yaml"; do
    tmpl="${pair%%:*}"; out="$here/$cloud/${pair#*:}"
    if ((check)); then
      if ! diff -q <(build "$cloud" "$name" "$tmpl") "$out" >/dev/null 2>&1; then
        echo "out of date: deploy/byoc/$cloud/${pair#*:} (run deploy/byoc/gen-cloud-init.sh)" >&2; rc=1
      fi
    else
      build "$cloud" "$name" "$tmpl" >"$out"
      echo "wrote deploy/byoc/$cloud/${pair#*:} ($(wc -c <"$out") bytes)"
    fi
  done
done
exit "$rc"
