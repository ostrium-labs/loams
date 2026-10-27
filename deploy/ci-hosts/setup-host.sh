#!/usr/bin/env bash
# Sets up a Loams local CI host (docs/guides/local-ci.md). Idempotent: run it
# again after any change, or to repair a host.
#
#   sudo deploy/ci-hosts/setup-host.sh [options]
#
# Roles:
#   primary  the NativeLink scheduler + cache (CAS, action cache) + a worker.
#   worker   a NativeLink worker that joins the primary over the tailnet.
#
# Options:
#   --role primary|worker      default: primary
#   --only LIST                comma-separated components (default: all):
#                              tailscale,toolchain,nativelink,sccache,podman,runner,monitor
#   --primary ADDR             worker role: the primary's Tailscale address (default:
#                              the one in the bundle)
#   --bundle FILE              worker role: credentials from the primary (--issue-bundle)
#   --issue-bundle NAME FILE   primary role: write a worker's credentials bundle, then exit
#   --tailscale-authkey-file F join the tailnet with this auth key when not connected
#   --tailscale-hostname NAME  default: loams-ci-a (primary) or loams-ci-b (worker)
#   --data-root DIR            caches and work directories (default /mnt/Projects/ci)
#   --nl-gb N                  NativeLink disk cap in GiB (default 20 primary, 80 worker)
#   --sccache-gb N             sccache store cap in GiB (primary only; default 8)
#   --jobs N                   concurrent NativeLink actions (default 4 primary, 8 worker)
#   --runner REPO[:N]          runners for REPO (repeatable): REPO alone is one runner
#                              <host>-<repo>; REPO:N is N runners <host>-<repo>-1..N
#                              (default: ostrium-labs/loams:2 and ostrium-labs/iggy on
#                              the primary, ostrium-labs/loams:3 on a worker)
#   --runners N                N runners for ostrium-labs/loams (keeps the other repos)
#   --gh-token-file FILE       a GitHub token that may mint runner registration tokens
#                              (classic repo scope, or fine-grained with Administration:
#                              read and write on the runners' repositories); --heal uses it
#   --store-gh-token           copy that token to /etc/loams-ci/github-token (root, 0600)
#                              and use the copy (for a host where FILE is temporary)
#   --reg-token REPO=FILE      a registration token minted elsewhere (valid 1 hour)
#   --runner-memory SIZE       memory cap per runner container (default: the RAM left
#                              after --runner-reserve-gb, split over all the runners)
#   --runner-reserve-gb N      RAM kept for the desktop and NativeLink (default 5 primary,
#                              1.5 worker): A 15 GiB primary gets 2 x 5 GiB runners, a 15 GiB
#                              worker 3 x 4.5 GiB
#   --runner-jobs N            CARGO_BUILD_JOBS per runner (default: one per
#                              RUNNER_GB_PER_JOB of its memory cap)
#   --runner-disk-gb N         free disk a job needs per ostrium-labs/loams runner (default 5
#                              primary, whose disk is shared with the owner's builds; 10 worker)
#   --min-free-gb N            override the total: a job refuses to start below this
#   --dry-run                  print the runner plan (names, units, sizes, disk floor,
#                              runners to retire) and change nothing
#   --heal                     re-register runners GitHub has dropped and restart dead ones,
#                              with the settings of the last full run (loams-ci-runner-heal.timer
#                              runs this every 5 minutes; needs --gh-token-file at setup)
#   --owner USER               the host's owner, who gets the client certificate
#                              (default: $SUDO_USER)
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd /
# shellcheck source=deploy/ci-hosts/versions.env
. "$HERE/versions.env"
# shellcheck source=deploy/ci-hosts/lib/common.sh
. "$HERE/lib/common.sh"
# shellcheck source=deploy/ci-hosts/lib/user.sh
. "$HERE/lib/user.sh"
# shellcheck source=deploy/ci-hosts/lib/tls.sh
. "$HERE/lib/tls.sh"
# shellcheck source=deploy/ci-hosts/lib/nativelink.sh
. "$HERE/lib/nativelink.sh"
# shellcheck source=deploy/ci-hosts/lib/sccache.sh
. "$HERE/lib/sccache.sh"
# shellcheck source=deploy/ci-hosts/lib/podman.sh
. "$HERE/lib/podman.sh"
# shellcheck source=deploy/ci-hosts/lib/runner.sh
. "$HERE/lib/runner.sh"
# shellcheck source=deploy/ci-hosts/lib/toolchain.sh
. "$HERE/lib/toolchain.sh"
# shellcheck source=deploy/ci-hosts/lib/bundle.sh
. "$HERE/lib/bundle.sh"
# shellcheck source=deploy/ci-hosts/lib/tailscale.sh
. "$HERE/lib/tailscale.sh"
# shellcheck source=deploy/ci-hosts/lib/monitor.sh
. "$HERE/lib/monitor.sh"

SETUP_ENV=/etc/loams-ci/setup.env
ROLE=primary
ONLY=all
PRIMARY=
DATA_ROOT=/mnt/Projects/ci
NL_GB=
NL_JOBS=
SCCACHE_GB=
RUNNER_REPOS=()
REG_TOKENS=()
GH_TOKEN_FILE=
RUNNER_MEMORY=
RUNNER_JOBS=
RUNNER_MIN_FREE_GB=
RUNNER_RESERVE_GB=
RUNNER_DISK_GB=
RUNNER_GB_PER_JOB=
HEAL=0
DRY_RUN=0
STORE_GH_TOKEN=0
# The settings of the previous run are the defaults of this one (an explicit
# option wins), so a re-run or --heal keeps e.g. --data-root and the caps.
if [[ -r $SETUP_ENV ]]; then
  # shellcheck disable=SC1090
  . "$SETUP_ENV"
  read -r -a RUNNER_REPOS <<<"${SAVED_RUNNER_REPOS:-}"
fi
CLI_REPOS=()
CLI_RUNNERS=
if [[ ${SETUP_ENV_VERSION:-1} -lt 2 ]]; then
  # Version 1 saved fixed per-runner sizes and one runner per repository;
  # version 2 sizes runners from RAM and defaults to several loams runners.
  RUNNER_MEMORY='' RUNNER_JOBS='' RUNNER_MIN_FREE_GB=''
  RUNNER_REPOS=()
fi
BUNDLE=
ISSUE_NAME=
ISSUE_OUT=
TS_AUTHKEY_FILE=
TS_HOSTNAME=
OWNER_USER=${SUDO_USER:-${OWNER_USER:-}}

while (($#)); do
  case $1 in
    --role) ROLE=$2; shift 2 ;;
    --only) ONLY=$2; shift 2 ;;
    --primary) PRIMARY=$2; shift 2 ;;
    --data-root) DATA_ROOT=$2; shift 2 ;;
    --nl-gb) NL_GB=$2; shift 2 ;;
    --jobs) NL_JOBS=$2; shift 2 ;;
    --sccache-gb) SCCACHE_GB=$2; shift 2 ;;
    --runner) CLI_REPOS+=("$2"); shift 2 ;;
    --gh-token-file) GH_TOKEN_FILE=$2; shift 2 ;;
    --reg-token) REG_TOKENS+=("$2"); shift 2 ;;
    --runners) CLI_RUNNERS=$2; shift 2 ;;
    --runner-reserve-gb) RUNNER_RESERVE_GB=$2; shift 2 ;;
    --runner-disk-gb) RUNNER_DISK_GB=$2; shift 2 ;;
    --runner-memory) RUNNER_MEMORY=$2; shift 2 ;;
    --runner-jobs) RUNNER_JOBS=$2; shift 2 ;;
    --min-free-gb) RUNNER_MIN_FREE_GB=$2; shift 2 ;;
    --owner) OWNER_USER=$2; shift 2 ;;
    --heal) HEAL=1; ONLY=runner; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --store-gh-token) STORE_GH_TOKEN=1; shift ;;
    --bundle) BUNDLE=$2; shift 2 ;;
    --issue-bundle) ISSUE_NAME=$2; ISSUE_OUT=$3; shift 3 ;;
    --tailscale-authkey-file) TS_AUTHKEY_FILE=$2; shift 2 ;;
    --tailscale-hostname) TS_HOSTNAME=$2; shift 2 ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;/^set -euo/d'; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

((${#CLI_REPOS[@]})) && RUNNER_REPOS=("${CLI_REPOS[@]}")
[[ $ROLE == primary || $ROLE == worker ]] || die "--role must be primary or worker"
((DRY_RUN)) || require_root
[[ -n $OWNER_USER && $OWNER_USER != root ]] || die "run with sudo from the owner's account, or pass --owner USER"
OWNER_HOME=$(getent passwd "$OWNER_USER" | cut -d: -f6)
[[ -d $OWNER_HOME ]] || die "no home directory for $OWNER_USER"

if [[ $ROLE == primary ]]; then
  : "${NL_GB:=20}" "${NL_JOBS:=4}" "${SCCACHE_GB:=8}"
  NL_MEMORY_HIGH=${NL_MEMORY_HIGH:-6G} NL_MEMORY_MAX=${NL_MEMORY_MAX:-8G}
  : "${RUNNER_RESERVE_GB:=5}" "${RUNNER_DISK_GB:=5}" "${RUNNER_GB_PER_JOB:=1.25}" "${TS_HOSTNAME:=loams-ci-a}"
  ((${#RUNNER_REPOS[@]})) || RUNNER_REPOS=(ostrium-labs/loams:2 ostrium-labs/iggy)
else
  : "${NL_GB:=80}" "${NL_JOBS:=8}" "${SCCACHE_GB:=0}"
  NL_MEMORY_HIGH=${NL_MEMORY_HIGH:-9G} NL_MEMORY_MAX=${NL_MEMORY_MAX:-11G}
  : "${RUNNER_RESERVE_GB:=1.5}" "${RUNNER_DISK_GB:=10}" "${RUNNER_GB_PER_JOB:=1.25}" "${TS_HOSTNAME:=loams-ci-b}"
  ((${#RUNNER_REPOS[@]})) || RUNNER_REPOS=(ostrium-labs/loams:3)
fi
if [[ -n $CLI_RUNNERS ]]; then
  [[ $CLI_RUNNERS =~ ^[1-9][0-9]*$ ]] || die "--runners needs a number"
  repos=()
  for r in "${RUNNER_REPOS[@]}"; do [[ ${r%%:*} == ostrium-labs/loams ]] || repos+=("$r"); done
  RUNNER_REPOS=("ostrium-labs/loams:$CLI_RUNNERS" "${repos[@]}")
fi

want() { [[ $ONLY == all || ,$ONLY, == *",$1,"* ]]; }

if ((DRY_RUN)); then
  runner_plan
  exit 0
fi

if [[ -n $ISSUE_NAME ]]; then
  issue_bundle "$ISSUE_NAME" "$ISSUE_OUT"
  exit 0
fi

if ((STORE_GH_TOKEN)) && [[ -n $GH_TOKEN_FILE ]]; then
  install -d -o root -g root -m 755 /etc/loams-ci
  install -o root -g root -m 600 "$GH_TOKEN_FILE" /etc/loams-ci/github-token
  GH_TOKEN_FILE=/etc/loams-ci/github-token
fi

if ((HEAL)); then
  [[ -r $SETUP_ENV ]] || die "--heal needs a previous full run of setup-host.sh"
  heal_runners
  exit 0
fi

pkg_install curl jq openssl ca-certificates tar gzip sudo debian:xz-utils arch:xz
want tailscale && setup_tailscale
ensure_ci_user
ensure_data_root
save_setup
if [[ $ROLE == worker ]]; then
  [[ -n $BUNDLE ]] && install_bundle "$BUNDLE"
  [[ -n $PRIMARY ]] || PRIMARY=$(cat /etc/loams-ci/primary 2>/dev/null || true)
  [[ -n $PRIMARY ]] || die "worker role: pass --bundle FILE (or --primary ADDR)"
  echo "$PRIMARY" >/etc/loams-ci/primary
fi

if want toolchain; then
  setup_sysctl
  setup_rust
  setup_node
fi
want nativelink && setup_nativelink
want sccache && setup_sccache
if want podman || want runner; then setup_podman; fi
want runner && setup_runners
want monitor && setup_monitor

log "done ($ROLE). Check with: /opt/loams-ci/bin/loams-ci-status"
