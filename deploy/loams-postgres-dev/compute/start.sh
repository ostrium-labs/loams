#!/usr/bin/env bash
# Minimal compute entrypoint, adapted from neondatabase/neon
# docker-compose/compute_wrapper/shell/compute.sh (Apache-2.0). The tenant and timeline are created by the caller
# (pg-bootstrap, Loams, or the README's curl calls) through the pageserver API, so this script
# needs no curl/jq: it fills the compute spec and starts compute_ctl.
#
# The ids: TENANT_ID and TIMELINE_ID from the environment when either is set, otherwise the ones
# pg-bootstrap stored in $BOOTSTRAP_IDS (compute1 mounts its volume read-only; compute2 does not,
# so a branch compute never falls back to the main timeline).
set -euo pipefail
BOOTSTRAP_IDS=${BOOTSTRAP_IDS:-/var/lib/pg-bootstrap/ids.env}
if [ -z "${TENANT_ID:-}" ] && [ -z "${TIMELINE_ID:-}" ] && [ -r "$BOOTSTRAP_IDS" ]; then
    TENANT_ID=$(sed -n 's/^TENANT_ID=//p' "$BOOTSTRAP_IDS")
    TIMELINE_ID=$(sed -n 's/^TIMELINE_ID=//p' "$BOOTSTRAP_IDS")
    echo "using the tenant and timeline pg-bootstrap stored in $BOOTSTRAP_IDS"
fi
: "${TENANT_ID:?set TENANT_ID (or let pg-bootstrap create one)}" "${TIMELINE_ID:?set TIMELINE_ID (or let pg-bootstrap create one)}"
echo "attaching to tenant ${TENANT_ID}, timeline ${TIMELINE_ID}"
until (exec 3<>/dev/tcp/pageserver/6400) 2>/dev/null; do echo "waiting for pageserver"; sleep 1; done
# A container that was stopped and started again keeps its /tmp, including the
# previous Postgres's socket lock (/tmp/.s.PGSQL.55433.lock). When the new
# Postgres happens to get the PID recorded there, it refuses to start ("Is
# another postmaster using socket file"), compute_ctl exits, and the compute
# never answers again (#1185; found by the DX0 compute nightly, 2026-10-28).
# Nothing else runs in this container yet, so the lock is always stale here.
rm -f /tmp/.s.PGSQL.* /var/db/postgres/compute/postmaster.pid
sed -e "s|TENANT_ID|${TENANT_ID}|" -e "s|TIMELINE_ID|${TIMELINE_ID}|" \
    /var/db/postgres/configs/config.json > /tmp/config.json
exec /usr/local/bin/compute_ctl --pgdata /var/db/postgres/compute \
     -C "postgresql://cloud_admin@localhost:55433/postgres" \
     -b /usr/local/bin/postgres \
     --compute-id "compute-${HOSTNAME}" \
     --config /tmp/config.json
