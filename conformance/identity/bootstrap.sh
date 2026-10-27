#!/usr/bin/env bash
# IA0 bootstrap: one org, one person, one agent, one OIDC app, one OpenFGA store with model v1.
#
# Runs against a fresh stack started by up.sh. It uses the first instance's IAM_OWNER machine
# user (its PAT is in .secrets/admin.pat, written by Zitadel at start-from-init) and Zitadel's
# public HTTP APIs only. It writes:
#   .secrets/agent-1.json   the agent's private-key JWT key (Zitadel's JSON key format)
#   .env.ia0                issuer, project id (the audience), client id, user ids, store id
set -euo pipefail
cd "$(dirname "$0")"
source .env

Z=${ZITADEL_URL:-http://localhost:${ZITADEL_PORT:-18080}}
FGA=${OPENFGA_URL:-http://127.0.0.1:${OPENFGA_HTTP_PORT:-18081}}
PAT=$(cat .secrets/admin.pat)

if [[ -f .env.ia0 ]]; then
  echo "bootstrap: .env.ia0 exists; this stack is already bootstrapped (down -v to start over)" >&2
  exit 0
fi

z() { # z METHOD PATH [JSON] [ORG]
  local args=(-sS --fail-with-body -X "$1" "$Z$2" -H "Authorization: Bearer $PAT" -H 'Content-Type: application/json')
  [[ -n "${4:-}" ]] && args+=(-H "x-zitadel-orgid: $4")
  [[ -n "${3:-}" ]] && args+=(-d "$3")
  curl "${args[@]}"
}

# 1. The org. Its id is the tuple id: org:<zitadel_org_id> (D1605).
ORG=$(z POST /v2/organizations '{"name":"acme"}' | jq -re .organizationId)

# 2. The project (the audience every Loams listener requires) and its OIDC app for the console.
PROJECT=$(z POST /management/v1/projects '{"name":"loams","projectRoleAssertion":false}' "$ORG" | jq -re .id)
APP=$(z POST "/management/v1/projects/$PROJECT/apps/oidc" "$(jq -nc '{
  name: "loams-console",
  redirectUris: ["http://127.0.0.1:18090/callback"],
  responseTypes: ["OIDC_RESPONSE_TYPE_CODE"],
  grantTypes: ["OIDC_GRANT_TYPE_AUTHORIZATION_CODE", "OIDC_GRANT_TYPE_REFRESH_TOKEN"],
  appType: "OIDC_APP_TYPE_USER_AGENT",
  authMethodType: "OIDC_AUTH_METHOD_TYPE_NONE",
  accessTokenType: "OIDC_TOKEN_TYPE_JWT",
  devMode: true
}')" "$ORG")
CLIENT_ID=$(jq -re .clientId <<<"$APP")

# 3. A person: alice, with a verified email and a password (no change required).
ALICE=$(z POST /v2/users/human "$(jq -nc --arg org "$ORG" --arg pw "$ALICE_PASSWORD" '{
  organization: {orgId: $org},
  username: "alice",
  profile: {givenName: "Alice", familyName: "Example"},
  email: {email: "alice@acme.example", isVerified: true},
  password: {password: $pw, changeRequired: false}
}')" | jq -re .userId)

# 4. An agent: a machine user whose access tokens are JWTs, with a private-key JWT key.
AGENT=$(z POST /management/v1/users/machine '{"userName":"agent-1","name":"Agent 1","accessTokenType":"ACCESS_TOKEN_TYPE_JWT"}' "$ORG" | jq -re .userId)
KEY_EXP=$(date -u -d '+30 days' +%Y-%m-%dT%H:%M:%SZ)
z POST "/management/v1/users/$AGENT/keys" "{\"type\":\"KEY_TYPE_JSON\",\"expirationDate\":\"$KEY_EXP\"}" "$ORG" \
  | jq -re .keyDetails | base64 -d > .secrets/agent-1.json
chmod 600 .secrets/agent-1.json

# 5. Short-lived access tokens: 15 minutes for every token this instance issues (D1607, §19).
z PUT /admin/v1/settings/oidc '{"accessTokenLifetime":"900s","idTokenLifetime":"900s","refreshTokenIdleExpiration":"86400s","refreshTokenExpiration":"604800s"}' >/dev/null

# 6. OpenFGA: a store, model v1 written by the OpenFGA CLI from fga.mod, and the resource tree.
STORE=$(curl -sS --fail-with-body -X POST "$FGA/stores" -d '{"name":"loams-ia0"}' | jq -re .id)
./fga.sh model write --store-id "$STORE" --file /model/fga.mod >/dev/null
MODEL=$(curl -sS --fail-with-body "$FGA/stores/$STORE/authorization-models?page_size=1" | jq -re '.authorization_models[0].id')

cat > .env.ia0 <<EOF2
IA0_ISSUER=$Z
IA0_AUDIENCE=$PROJECT
IA0_CLIENT_ID=$CLIENT_ID
IA0_ORG=$ORG
IA0_ALICE=$ALICE
IA0_AGENT=$AGENT
IA0_AGENT_KEY=.secrets/agent-1.json
IA0_FGA_URL=$FGA
IA0_FGA_GRPC=http://127.0.0.1:${OPENFGA_GRPC_PORT:-18082}
IA0_STORE=$STORE
IA0_MODEL=$MODEL
EOF2
echo "bootstrap: org=$ORG project=$PROJECT client=$CLIENT_ID alice=$ALICE agent=$AGENT store=$STORE model=$MODEL"
