#!/usr/bin/env bash
# Health check for a Loams local CI host (docs/guides/local-ci.md).
#
#   deploy/ci-hosts/status.sh             report; exit 1 when a check fails (warnings,
#                                         such as low disk, are reported but exit 0)
#   deploy/ci-hosts/status.sh --json      the same as one JSON object
#   deploy/ci-hosts/status.sh --github    also ask GitHub whether the runners are online (gh)
#
# Works without sudo: cache sizes come from du when readable (root, or the ci
# user), else from /run/loams-ci/sizes, which loams-ci-monitor.timer refreshes
# every 15 minutes. Never prints a secret.
set -uo pipefail

# setup-host.sh records the host's data root and the jobs' disk floor here.
DATA_ROOT=${DATA_ROOT:-$(sed -n 's/^DATA_ROOT=//p' /opt/loams-ci/status.env 2>/dev/null)}
DATA_ROOT=${DATA_ROOT:-/mnt/Projects/ci}
CI_USER=${CI_USER:-ci}
SIZES=/run/loams-ci/sizes
JSON=0
GITHUB=0
for a in "$@"; do
  case $a in
    --json) JSON=1 ;;
    --github) GITHUB=1 ;;
    -h | --help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

rows=()
problems=0
warnings=0
add() {
  rows+=("$1|$2|$3")
  case $2 in
    ok) ;;
    warn) warnings=$((warnings + 1)) ;;
    *) problems=$((problems + 1)) ;;
  esac
}

# Size of a directory in GiB (one decimal), or empty when unknown.
size_gib() {
  local dir=$1 bytes=
  if [[ -r $dir && -x $dir ]]; then
    bytes=$(du -sb "$dir" 2>/dev/null | cut -f1)
  elif [[ -r $SIZES ]]; then
    bytes=$(awk -v d="$dir" '$1 == "size" && $2 == d {print $3}' "$SIZES")
  fi
  [[ -n $bytes ]] && awk -v b="$bytes" 'BEGIN{printf "%.1f", b/1073741824}'
}

# cache NAME DIR CAP_GIB
cache() {
  local name=$1 dir=$2 cap=$3 used pct
  used=$(size_gib "$dir")
  if [[ -z $used ]]; then
    add "$name" ok "size unknown (run with sudo, or wait for loams-ci-monitor.timer)"
    return
  fi
  if [[ -z $cap || $cap == 0 ]]; then
    add "$name" ok "$used GiB"
    return
  fi
  pct=$(awk -v u="$used" -v c="$cap" 'BEGIN{printf "%d", 100*u/c}')
  if ((pct > 110)); then
    add "$name" warn "$used / $cap GiB ($pct%, over its cap: is eviction running?)"
  else
    add "$name" ok "$used / $cap GiB ($pct%)"
  fi
}

# Disk: the data root's filesystem and / (once each), against the jobs' floor.
floor=$(sed -n 's/^MIN_FREE_GB=//p' /opt/loams-ci/status.env 2>/dev/null)
floor=${floor:-10}
seen=
for fs in "$DATA_ROOT" /; do
  [[ -e $fs ]] || continue
  mnt=$(df --output=target "$fs" | tail -n1)
  [[ " $seen " == *" $mnt "* ]] && continue
  seen+=" $mnt"
  free=$(df -BG --output=avail "$fs" | tail -n1 | tr -dc '0-9')
  pct=$(df --output=pcent "$fs" | tail -n1 | tr -dc '0-9')
  if [[ $fs == "$DATA_ROOT" ]] && ((free < floor)); then
    add "disk $mnt" warn "${free} GiB free (${pct}% used): local CI jobs refuse to start below ${floor} GiB"
  elif ((free < 5)); then
    add "disk $mnt" warn "${free} GiB free (${pct}% used)"
  else
    add "disk $mnt" ok "${free} GiB free (${pct}% used)"
  fi
done

ts_ip=$(tailscale ip -4 2>/dev/null | head -n1)
if [[ -n $ts_ip ]]; then add tailscale ok "$ts_ip"; else add tailscale warn "not connected: other hosts cannot reach this one"; fi

if [[ -f /etc/systemd/system/loams-nativelink.service ]]; then
  st=$(systemctl is-active loams-nativelink 2>/dev/null)
  role=$(sed -n 's/^Description=.*NativeLink (\(.*\))$/\1/p' /etc/systemd/system/loams-nativelink.service)
  health=$(curl -fsS -m 3 http://127.0.0.1:50061/status 2>/dev/null || curl -fsS -m 3 http://127.0.0.1:50071/status 2>/dev/null)
  if [[ $st != active ]]; then
    add "nativelink ($role)" fail "$st"
  elif [[ $health == *'"Ok"'* && $health == *registered* ]]; then
    add "nativelink ($role)" ok "active; worker registered with the scheduler"
  else
    add "nativelink ($role)" fail "active, but health is not ok: ${health:-no answer}"
  fi
  if [[ -r /etc/loams-ci/nativelink.json5 ]]; then
    cap=$(awk 'match($0, /max_bytes: *[0-9]+/) {v = substr($0, RSTART, RLENGTH); gsub(/[^0-9]/, "", v); s += v} END {if (s) printf "%.0f", s/1073741824}' /etc/loams-ci/nativelink.json5)
  else
    cap=$(awk '$1 == "cap" && $2 == "nativelink" {print $3}' "$SIZES" 2>/dev/null)
  fi
  cache "nativelink cache" "$DATA_ROOT/nativelink" "$cap"
fi

if [[ -f /etc/systemd/system/loams-sccache-store.service ]]; then
  st=$(systemctl is-active loams-sccache-store 2>/dev/null)
  code=$(curl -s -o /dev/null -m 3 -w '%{http_code}' http://127.0.0.1:50081/ 2>/dev/null)
  if [[ $st == active && $code == 401 ]]; then
    add "sccache store" ok "active, password required"
  else
    add "sccache store" fail "$st (http $code)"
  fi
  cap=$(sed -n 's|^ExecStart=.*sccache-evict [^ ]* \([0-9]*\)$|\1|p' /etc/systemd/system/loams-sccache-evict.service 2>/dev/null)
  cache "sccache cache" "$DATA_ROOT/sccache" "$cap"
  st=$(systemctl is-active loams-sccache-evict.timer 2>/dev/null)
  if [[ $st == active ]]; then add "sccache eviction timer" ok active; else add "sccache eviction timer" fail "$st"; fi
elif [[ -f /etc/loams-ci/sccache.env ]]; then
  add "sccache" ok "client of the primary's store"
fi

# Runners: each is a `podman run --name loams-runner-NAME` of the ci user with a
# Runner.Listener inside (visible in the host's process list).
{
  ps -u "$CI_USER" -o args= 2>/dev/null | sed -n 's/^.*podman run .*--name loams-runner-\([^ ]*\) .*$/\1/p'
  if [[ -r /home/$CI_USER/.config/systemd/user ]]; then
    sed -n 's/^.*--name loams-runner-\([^ ]*\) .*$/\1/p' /home/"$CI_USER"/.config/systemd/user/loams-runner-*.service 2>/dev/null
  elif [[ -r $SIZES ]]; then
    awk '$1 == "runner" {print $2}' "$SIZES"
  fi
} | sort -u >"${TMPDIR:-/tmp}/loams-ci-runners.$$"
mapfile -t runners <"${TMPDIR:-/tmp}/loams-ci-runners.$$"
rm -f "${TMPDIR:-/tmp}/loams-ci-runners.$$"
for name in "${runners[@]}"; do
  if pgrep -u "$CI_USER" -f "/runners/$name/runner/bin/Runner.Listener" >/dev/null 2>&1; then
    if pgrep -u "$CI_USER" -f "/runners/$name/runner/bin/Runner.Worker" >/dev/null 2>&1; then
      add "runner $name" ok "running a job"
    else
      add "runner $name" ok "idle, listening"
    fi
  else
    add "runner $name" fail "not running (sudo systemctl --user -M $CI_USER@ status 'loams-runner-*')"
  fi
done
if ((${#runners[@]})); then
  mem=$(sed -n 's/^RUNNER_MEMORY=//p' /opt/loams-ci/status.env 2>/dev/null)
  jobs=$(sed -n 's/^RUNNER_JOBS=//p' /opt/loams-ci/status.env 2>/dev/null)
  add "runner sizing" ok "${#runners[@]} runners, each ${mem:-?} memory and ${jobs:-?} cargo jobs; jobs need ${floor} GiB free"
fi
if [[ -f /etc/systemd/system/loams-ci-runner-heal.timer ]]; then
  st=$(systemctl is-active loams-ci-runner-heal.timer 2>/dev/null)
  last=$(systemctl show -p Result --value loams-ci-runner-heal.service 2>/dev/null)
  if [[ $st == active && ${last:-success} == success ]]; then
    add "runner self-healing" ok "timer active, last run ${last:-not yet}"
  else
    add "runner self-healing" fail "timer $st, last run $last (journalctl -u loams-ci-runner-heal)"
  fi
fi
cache "runner work + caches" "$DATA_ROOT/runners" ""
cache "tool/cargo/rustup cache" "$DATA_ROOT/cache" ""
cache "runner images" "$DATA_ROOT/containers" ""

if ((GITHUB)) && command -v gh >/dev/null 2>&1; then
  for repo in ostrium-labs/loams ostrium-labs/iggy; do
    while IFS='|' read -r name status busy; do
      [[ -n $name ]] || continue
      if [[ $status == online ]]; then add "github $repo $name" ok "online, busy=$busy"; else add "github $repo $name" warn "$status"; fi
    done < <(gh api "repos/$repo/actions/runners" --jq '.runners[] | select(.labels | map(.name) | index("loams-local")) | "\(.name)|\(.status)|\(.busy)"' 2>/dev/null)
  done
  var=$(gh variable get CI_RUNNER --repo ostrium-labs/loams 2>/dev/null || echo unset)
  add "CI_RUNNER (loams)" ok "$var"
fi

# Listening sockets: only loopback and the tailnet address.
bad=$(ss -Hltn 2>/dev/null | awk '{print $4}' | grep -E ':(50051|50061|50071|50081)$' | grep -vE "^(127\.0\.0\.1|\[::1\]|${ts_ip:-none}):" || true)
if [[ -z $bad ]]; then add "bind addresses" ok "loopback + tailnet only"; else add "bind addresses" fail "unexpected: $(echo "$bad" | tr '\n' ' ')"; fi

if ((JSON)); then
  printf '{"host":"%s","time":"%s","ok":%s,"warnings":%d,"checks":[' "$(hostname -s)" "$(date -u +%FT%TZ)" "$( ((problems == 0)) && echo true || echo false)" "$warnings"
  sep=
  for r in "${rows[@]}"; do
    IFS='|' read -r name state detail <<<"$r"
    printf '%s{"name":"%s","state":"%s","detail":"%s"}' "$sep" "$name" "$state" "${detail//\"/\'}"
    sep=,
  done
  printf ']}\n'
else
  printf '%-34s %-5s %s\n' CHECK STATE DETAIL
  for r in "${rows[@]}"; do
    IFS='|' read -r name state detail <<<"$r"
    printf '%-34s %-5s %s\n' "$name" "$state" "$detail"
  done
fi
((problems == 0))
