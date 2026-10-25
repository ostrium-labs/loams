#!/usr/bin/env bash
# IA0: start Zitadel (profile identity) and OpenFGA (profile authz) on loopback, then bootstrap.
# Writes the git-ignored .env (secrets) on first run and .env.ia0 (issuer, ids) from bootstrap.sh.
set -euo pipefail
cd "$(dirname "$0")"

compose() {
  if command -v docker >/dev/null 2>&1; then docker compose "$@"; else podman compose "$@"; fi
}

rand() { tr -dc 'A-Za-z0-9' </dev/urandom | head -c "$1"; }

if [[ ! -f .env ]]; then
  umask 077
  {
    echo "ZITADEL_MASTERKEY=$(rand 32)"
    echo "ZITADEL_DB_PASSWORD=$(rand 32)"
    # Zitadel's default password policy wants upper, lower, digit and symbol.
    echo "ZITADEL_ADMIN_PASSWORD=Ia0-$(rand 20)-x9!"
    echo "LOGIN_SESSION_COOKIE_SECRET=$(rand 48)"
    echo "ALICE_PASSWORD=Al1ce-$(rand 20)-!"
    echo "IA0_ADMIN_TOKEN=$(rand 40)"
  } > .env
fi
mkdir -p .secrets results
chmod 700 .secrets

compose --profile identity --profile authz up -d --wait "$@"
[[ "${IA0_SKIP_BOOTSTRAP:-}" == 1 ]] || ./bootstrap.sh
