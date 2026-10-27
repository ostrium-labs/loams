#!/usr/bin/env bash
# IA0 Task 4: register the spike's `identity-sync` endpoint as an Actions v2 webhook target and
# point event executions at it. The target's signing key goes to .secrets/events.key, which the
# spike service reads to verify ZITADEL-Signature.
set -euo pipefail
cd "$(dirname "$0")"
Z=${ZITADEL_URL:-http://localhost:${ZITADEL_PORT:-18080}}
PAT=$(cat .secrets/admin.pat)
api() { curl -sS --fail-with-body -X "$1" "$Z$2" -H "Authorization: Bearer $PAT" -H 'Content-Type: application/json' -d "$3"; }

if [[ ! -s .secrets/events.target ]]; then
  # interruptOnError=false: an event execution never blocks the API call that caused it.
  res=$(api POST /v2/actions/targets '{"name":"identity-sync","restWebhook":{"interruptOnError":false},"endpoint":"http://ia0-authz:18090/zitadel/events","timeout":"10s"}')
  (umask 077; jq -r .signingKey <<<"$res" > .secrets/events.key; jq -r .id <<<"$res" > .secrets/events.target)
fi
target=$(cat .secrets/events.target)

# Design 76 §8.1's events, plus user grants (finding F3) and machine-user creation.
for ev in org.member.added org.member.removed org.member.cascade.removed \
          user.grant.added user.grant.removed user.grant.cascade.removed \
          user.machine.added user.deactivated user.reactivated user.locked user.unlocked user.removed; do
  api PUT /v2/actions/executions "{\"condition\":{\"event\":{\"event\":\"$ev\"}},\"targets\":[\"$target\"]}" >/dev/null
done
echo "actions: event executions point at target $target"
