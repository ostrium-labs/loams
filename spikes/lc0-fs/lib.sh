# shellcheck shell=bash
# LC0 Task 2 spike helpers. Source this file; never run it.
#
# Machine A = this Linux host, with its own HOME ($LC0_ROOT/a) so its ~/.ti
#             state is isolated from other agents' ~/.ti.
# Machine B = a rootless podman container (image lc0-fs-b) with /dev/fuse and
#             its own HOME (/home/b, bind-mounted from $LC0_ROOT/b).
#
# Secrets: API keys come from ~/tidbcloud-lc0.env, sourced only inside the
# function that needs them. File-system tokens live in 0600 files under
# $LC0_SECRETS and reach processes only through TI_FS_TOKEN in the
# environment, never through flags. Never print them; pipe output through
# scrub.

set -o pipefail

REAL_HOME=${REAL_HOME:-/home/dinakaran}
LC0_ROOT=${LC0_ROOT:-$REAL_HOME/lc0-fs}
LC0_SECRETS=${LC0_SECRETS:?set LC0_SECRETS to a 0700 directory (the agent scratchpad)}
LC0_REGION=aws-us-east-1
LC0_IMAGE=${LC0_IMAGE:-localhost/lc0-fs-b:latest}
TI_BIN=$REAL_HOME/.ti/bin/ti
D9_BIN=$REAL_HOME/.ti/bin/ti-drive9
HOME_A=$LC0_ROOT/a
HOME_B_HOST=$LC0_ROOT/b
MNT_A=$HOME_A/mnt
export TI_TELEMETRY=off

# Remove anything that looks like a credential from text on stdin.
scrub() {
  sed -E \
    -e 's/eyJ[A-Za-z0-9._-]{10,}/<jwt>/g' \
    -e 's/("[A-Za-z_]*(token|key|secret|password|jwt)[A-Za-z_]*"): *"[^"]*"/\1:"<redacted>"/Ig' \
    -e 's/(fs_[A-Za-z0-9]{16,})/<fs-token>/g'
}

now() { date +%s.%N; }
elapsed() { awk -v a="$1" -v b="$2" 'BEGIN{printf "%.3f", b-a}'; }

fs_id() { cat "$LC0_SECRETS/fs_id"; }

# ti on machine A with the locally stored owner credential.
tiA() { HOME=$HOME_A "$TI_BIN" "$@"; }

# ti on machine A with a token from a 0600 file, passed via TI_FS_TOKEN.
tiA_tok() {
  local tokfile=$1; shift
  HOME=$HOME_A TI_FS_TOKEN=$(cat "$tokfile") TI_REGION_CODE=$LC0_REGION "$TI_BIN" "$@"
}

# ti with TiDB Cloud API keys (control-plane calls only).
tiAPI() {
  ( set -a; . "$REAL_HOME/tidbcloud-lc0.env"; set +a
    HOME=$LC0_ROOT/apihome "$TI_BIN" "$@" )
}

# drive9's admin tenant API: per-file-system quota and usage, by API key.
# Undocumented path: ti's companion runtime, not a documented ti command.
fs_quota() {
  ( set -a; . "$REAL_HOME/tidbcloud-lc0.env"; set +a
    export DRIVE9_PUBLIC_KEY=$TIDB_CLOUD_PUBLIC_KEY DRIVE9_PRIVATE_KEY=$TIDB_CLOUD_PRIVATE_KEY
    unset TIDB_CLOUD_PUBLIC_KEY TIDB_CLOUD_PRIVATE_KEY
    HOME=$LC0_ROOT/apihome "$D9_BIN" admin tenant get --tenant-id "$(fs_id)" \
      --region-code "$LC0_REGION" --json )
}

# Org-level bill for a month from the TiDB Cloud Billing API (digest auth;
# the key pair goes to curl through a config on stdin, not argv).
bill() {
  local month=${1:-$(date -u +%Y-%m)} what=${2:-bills}
  ( set -a; . "$REAL_HOME/tidbcloud-lc0.env"; set +a
    printf 'user = "%s:%s"\n' "$TIDB_CLOUD_PUBLIC_KEY" "$TIDB_CLOUD_PRIVATE_KEY" |
      curl -s --digest -K - "https://billing.tidbapi.com/v1beta1/$what/$month" )
}

B_CTR=lc0-fs-b

# Start machine B: a long-lived container with its own HOME and /dev/fuse.
b_up() {
  podman container exists "$B_CTR" && return 0
  mkdir -p "$HOME_B_HOST"
  # --init reaps exited mount processes; without it, `ti fs unmount` waits
  # on zombies and times out.
  podman run -d --init --name "$B_CTR" --device /dev/fuse --cap-add SYS_ADMIN \
    --security-opt label=disable --security-opt apparmor=unconfined \
    -e HOME=/home/b -e TI_TELEMETRY=off -e TI_REGION_CODE=$LC0_REGION \
    -v "$HOME_B_HOST:/home/b" -v "$REAL_HOME/.ti/bin:/opt/ti/bin:ro" \
    -w /home/b "$LC0_IMAGE" sleep infinity >/dev/null
}
b_down() { podman rm -f -t 0 "$B_CTR" >/dev/null 2>&1 || true; }

# Run a shell snippet on machine B. With a token file as $2 the token goes
# in as TI_FS_TOKEN (podman reads the value from this process's env).
tiB_sh() {
  local snippet=$1 tokfile=${2:-}
  if [ -n "$tokfile" ]; then
    TI_FS_TOKEN=$(cat "$tokfile") podman exec -i -e TI_FS_TOKEN "$B_CTR" bash -c "$snippet"
  else
    podman exec -i "$B_CTR" bash -c "$snippet"
  fi
}

json() { python3 -I -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
