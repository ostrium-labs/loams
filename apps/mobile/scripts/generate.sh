#!/usr/bin/env bash
# Regenerates every client from proto/ with buf (remote plugins on buf.build):
#   android/proto/src/generated               protobuf-javalite + connect-kotlin
#   ios/Packages/LoamsProto/.../Generated     swift-protobuf + connect-swift
#   mock/gen                                  protobuf-go + connect-go
# The output is committed; CI reruns this and fails on any difference.
set -euo pipefail
cd "$(dirname "$0")/.."
buf lint
# buf.build rate-limits anonymous remote-plugin use (resource_exhausted). Retry
# with back-off; set BUF_TOKEN (a free buf.build account) to lift the limit.
gen() {
  local t=$1 delay=20 out
  for attempt in 1 2 3 4 5; do
    if out=$(buf generate --template "buf.gen.$t.yaml" 2>&1); then
      return 0
    fi
    if [[ "$out" != *resource_exhausted* ]]; then
      echo "$out" >&2
      return 1
    fi
    echo "buf.build rate limit on $t (attempt $attempt); retrying in ${delay}s" >&2
    sleep "$delay"
    delay=$((delay * 2))
  done
  echo "$out" >&2
  return 1
}
for t in kotlin swift go; do
  gen "$t"
done
echo "generated from proto/ (tree $(scripts/proto-hash.sh))"
