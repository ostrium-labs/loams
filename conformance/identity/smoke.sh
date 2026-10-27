#!/usr/bin/env bash
# IA0 smoke (CI job identity-smoke): bring the stack up from nothing, bootstrap it, check that
# Zitadel's discovery document and OpenFGA's health answer, run the model tests and the
# cross-org scan, and, when IA0_SPIKE_BIN points at a built spikes/ia0-authz binary, run the
# end-to-end allow/deny scenario and one Actions v2 delivery. Always brings the stack down.
set -euo pipefail
cd "$(dirname "$0")"
compose() {
  if command -v docker >/dev/null 2>&1; then docker compose "$@"; else podman compose "$@"; fi
}
profiles=(--profile identity --profile authz --profile spike)
cleanup() { compose "${profiles[@]}" down -v >/dev/null 2>&1 || true; rm -rf .secrets .env.ia0; }
trap cleanup EXIT

# Secrets must never be committable.
for f in .env .env.ia0 .secrets/admin.pat; do
  git check-ignore -q "$f" || { echo "smoke: $f is not git-ignored" >&2; exit 1; }
done

cleanup
start=$(date +%s)
./up.sh
echo "smoke: stack up and bootstrapped in $(( $(date +%s) - start )) s"
source .env.ia0
curl -fsS "$IA0_ISSUER/.well-known/openid-configuration" | jq -e '.issuer and .jwks_uri' >/dev/null
curl -fsS "$IA0_FGA_URL/healthz" | jq -e '.status == "SERVING"' >/dev/null
echo "smoke: discovery and OpenFGA health ok"

./fga.sh model test --tests /model/core.fga.yaml
python3 -I cross_org_scan.py --self-test

if [[ -n "${IA0_SPIKE_BIN:-}" ]]; then
  export IA0_SPIKE_BIN
  compose "${profiles[@]}" up -d --wait ia0-authz
  ./actions.sh
  set -a; source .env; set +a
  (cd ../.. && IA0_FGA_GRPC="http://127.0.0.1:${OPENFGA_GRPC_PORT:-18082}" "$IA0_SPIKE_BIN" scenario)
  (cd ../.. && IA0_FGA_GRPC="http://127.0.0.1:${OPENFGA_GRPC_PORT:-18082}" "$IA0_SPIKE_BIN" events 5 500)
  python3 -I cross_org_scan.py --store "$IA0_FGA_URL" "$IA0_STORE"
fi
echo "smoke: ok"
