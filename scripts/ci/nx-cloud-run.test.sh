#!/usr/bin/env bash
# Self-test of scripts/ci/nx-cloud-run.sh with a fake `nx`: an Nx Cloud failure is retried once
# without the cloud; a task failure is not; a run already off the cloud is not.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/fake-nx" <<'EOF'
#!/usr/bin/env bash
echo "call NX_NO_CLOUD=${NX_NO_CLOUD:-unset}" >> "$FAKE_CALLS"
case "$FAKE_MODE" in
  cloud) [ "${NX_NO_CLOUD:-}" = true ] && exit 0
         echo ' NX   Nx Cloud: Workspace is unable to be authorized. Exiting run.'; exit 1 ;;
  credits) [ "${NX_NO_CLOUD:-}" = true ] && exit 0
         echo ' NX   Nx Cloud: this organization is out of credits for the month.'; exit 1 ;;
  task) echo ' NX   Running target test for project x failed'; exit 1 ;;
  ok) exit 0 ;;
esac
EOF
chmod +x "$tmp/fake-nx"

fail=0
check() { # name mode no_cloud want_status want_calls
  local name=$1 mode=$2 no_cloud=$3 want_status=$4 want_calls=$5 status=0
  : > "$tmp/calls"
  FAKE_MODE=$mode FAKE_CALLS="$tmp/calls" NX_NO_CLOUD=$no_cloud NX_CLOUD_RUN_NX="$tmp/fake-nx" \
    "$here/nx-cloud-run.sh" run-many -t test > "$tmp/out" 2>&1 || status=$?
  local calls; calls=$(wc -l < "$tmp/calls")
  if [ "$status" -ne "$want_status" ] || [ "$calls" -ne "$want_calls" ]; then
    echo "FAIL $name: status $status (want $want_status), calls $calls (want $want_calls)"; cat "$tmp/out"; fail=1
  else
    echo "ok   $name"
  fi
}
check 'cloud auth failure retried without the cloud' cloud false 0 2
check 'cloud credits failure retried without the cloud' credits false 0 2
check 'task failure not retried' task false 1 1
check 'success runs once' ok false 0 1
check 'already off the cloud: no retry' cloud true 0 1
check 'task failure off the cloud' task true 1 1
grep -q 'Nx Cloud unavailable' <(FAKE_MODE=cloud FAKE_CALLS=/dev/null NX_NO_CLOUD=false NX_CLOUD_RUN_NX="$tmp/fake-nx" "$here/nx-cloud-run.sh" 2>&1) \
  && echo 'ok   the retry leaves a warning' || { echo 'FAIL no warning'; fail=1; }
exit $fail
