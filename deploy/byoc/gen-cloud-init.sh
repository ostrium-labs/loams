#!/usr/bin/env bash
# Builds deploy/byoc/<cloud>/cloud-init.yaml from cloud-init.tmpl.yaml, lib/*
# and <cloud>/config.env, so the scripts a VM runs are the files in lib/ (which
# the linters and the tests cover) and the cloud-init files cannot drift.
#
#   gen-cloud-init.sh            rewrite the three files
#   gen-cloud-init.sh --check    fail if any file is out of date (CI)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
check=0; [[ "${1:-}" == "--check" ]] && check=1

indent() { sed 's/^\(.\)/      \1/' "$1"; }

build() {  # $1 = cloud, $2 = display name
  local cloud="$1" name="$2" line
  while IFS= read -r line || [[ -n "$line" ]]; do
    case "$line" in
      @@CONFIG@@)    indent "$here/$cloud/config.env" ;;
      @@KEY@@)       indent "$repo/docs/release/loams-release-key.asc" ;;
      @@BOOTSTRAP@@) indent "$here/lib/loams-byoc-bootstrap" ;;
      @@SECRETS@@)   indent "$here/lib/loams-byoc-secrets" ;;
      @@UNIT@@)      indent "$here/lib/loams-byoc.service" ;;
      @@CONFLUENT_UNIT@@)  indent "$here/lib/loams-byoc-confluent.service" ;;
      @@CONFLUENT_TIMER@@) indent "$here/lib/loams-byoc-confluent.timer" ;;
      *) line="${line//@@CLOUD_NAME@@/$name}"; printf '%s\n' "${line//@@CLOUD@@/$cloud}" ;;
    esac
  done <"$here/cloud-init.tmpl.yaml"
}

rc=0
for spec in "aws:AWS" "gcp:Google Cloud" "azure:Azure"; do
  cloud="${spec%%:*}"; name="${spec#*:}"
  out="$here/$cloud/cloud-init.yaml"
  if ((check)); then
    if ! diff -q <(build "$cloud" "$name") "$out" >/dev/null 2>&1; then
      echo "out of date: deploy/byoc/$cloud/cloud-init.yaml (run deploy/byoc/gen-cloud-init.sh)" >&2; rc=1
    fi
  else
    build "$cloud" "$name" >"$out"
    echo "wrote deploy/byoc/$cloud/cloud-init.yaml ($(wc -c <"$out") bytes)"
  fi
done
exit "$rc"
