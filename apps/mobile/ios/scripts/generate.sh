#!/usr/bin/env bash
# AP3 Task 1: regenerates the Swift side only (LoamsProto: SwiftProtobuf messages and
# connect-swift clients) from apps/mobile/proto with buf.gen.swift.yaml. It first checks
# that proto/ is the tree conformance/proto-ref.lock pins (the lock Android shares), so a
# generation never silently moves the contract. ../../scripts/generate.sh regenerates all
# three clients; CI's drift check reruns that one.
#
# The output is committed (docs/ios.md ruling E2); never edit it by hand.
set -euo pipefail
cd "$(dirname "$0")/../.."   # apps/mobile

want=$(sed -n 's/^tree_sha256=//p' conformance/proto-ref.lock)
have=$(scripts/proto-hash.sh)
if [[ "$want" != "$have" ]]; then
  echo "proto/ hashes to $have but conformance/proto-ref.lock pins $want;" >&2
  echo "update the lock with scripts/update-lock.sh first." >&2
  exit 1
fi

buf lint
# buf.build rate-limits anonymous remote-plugin use (resource_exhausted); retry with
# back-off. BUF_TOKEN (a free buf.build account) lifts the limit.
delay=20
for attempt in 1 2 3 4 5; do
  if out=$(buf generate --template buf.gen.swift.yaml 2>&1); then
    echo "generated LoamsProto from proto/ (tree $have)"
    exit 0
  fi
  if [[ "$out" != *resource_exhausted* ]]; then
    echo "$out" >&2
    exit 1
  fi
  echo "buf.build rate limit (attempt $attempt); retrying in ${delay}s" >&2
  sleep "$delay"
  delay=$((delay * 2))
done
echo "$out" >&2
exit 1
