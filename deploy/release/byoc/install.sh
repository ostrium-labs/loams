#!/usr/bin/env bash
# Installs the Loams preview stack on a machine of your own (BYOC) over SSH:
# installs Docker if it is missing, copies the compose bundle, writes .env
# (asking for the Tailscale auth key) and starts the stack. The
# machine needs no inbound port: the engine is reached over your tailnet.
#
#   deploy/release/byoc/install.sh [options] user@host
#
#   -p, --port N            SSH port [default: 22]
#   -i, --identity FILE     SSH private key
#   --dir DIR               install directory on the host, relative to the SSH
#                           user's home unless absolute [default: loams]
#   --authkey KEY           tailnet auth key [default: $TS_AUTHKEY, else asked]
#   --login-server URL      Headscale URL (coming soon, unsupported in 0.1.0-preview.2);
#                           [default: $TS_LOGIN_SERVER, else empty = Tailscale]
#   --headscale URL         (coming soon, unsupported in 0.1.0-preview.2) also run Headscale on the host (`--profile
#                           headscale`) at this public URL, mint the auth key
#                           there, and serve the engine over http
#   --version V             image tag [default: 0.1.0]
#   --all                   start every service, the Postgres stack included
#   --services "A B"        start these services (and what they depend on)
#                           [default: loams tailscale tidb]
#   --reconfigure           rewrite an existing .env on the host
#   -y, --yes               do not ask; take keys from flags or the environment
#
# Uninstall with byoc/uninstall.sh.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
release="$(cd "$here/.." && pwd)"
deploy="$(cd "$release/.." && pwd)"

port=22
identity=""
dir="loams"
authkey="${TS_AUTHKEY:-}"
login_server="${TS_LOGIN_SERVER-}"
headscale=""
version="0.1.0"
all=0
services="loams tailscale tidb"
reconfigure=0
assume_yes=0
target=""

die() { echo "install.sh: $*" >&2; exit 1; }
say() { echo "==> $*"; }

while (($#)); do
  case "$1" in
    -p|--port) port="${2:?}"; shift 2 ;;
    -i|--identity) identity="${2:?}"; shift 2 ;;
    --dir) dir="${2:?}"; shift 2 ;;
    --authkey) authkey="${2:?}"; shift 2 ;;
    --login-server) login_server="${2-}"; shift 2 ;;
    --headscale) headscale="${2:?}"; shift 2 ;;
    --version) version="${2:?}"; shift 2 ;;
    --all) all=1; shift ;;
    --services) services="${2:?}"; shift 2 ;;
    --reconfigure) reconfigure=1; shift ;;
    -y|--yes) assume_yes=1; shift ;;
    -h|--help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) die "unknown option: $1" ;;
    *) [[ -z "$target" ]] || die "one target only"; target="$1"; shift ;;
  esac
done
[[ -n "$target" ]] || die "usage: install.sh [options] user@host (see --help)"
[[ "$port" =~ ^[0-9]+$ ]] || die "--port must be a number"
[[ "$dir" =~ ^[A-Za-z0-9._/-]+$ ]] || die "--dir may hold only letters, digits and ._/-"
[[ "$dir" != *..* && "$dir" != "." && "$dir" != "./" && "${dir%/}" != "" ]] || die "--dir $dir is not an install directory"
[[ "$services" =~ ^[a-z0-9_\ -]*$ ]] || die "--services takes service names"
[[ "$version" =~ ^[A-Za-z0-9._-]+$ ]] || die "--version is not an image tag"
[[ -f "$release/compose.yaml" && -f "$deploy/loams-postgres-dev/compose.yaml" ]] \
  || die "run from a Loams checkout: $release/compose.yaml or the Postgres stack is missing"

# One SSH connection for every step, so a password or key passphrase is asked once.
ctl="${XDG_RUNTIME_DIR:-$HOME/.ssh}/loams-install-%C"
ssh_opts=(-p "$port" -o ControlMaster=auto -o "ControlPath=$ctl" -o ControlPersist=120)
[[ -n "$identity" ]] && ssh_opts+=(-i "$identity")
# Arguments are expanded here on purpose: callers quote them with printf %q.
# shellcheck disable=SC2029
ssh_cmd() { ssh "${ssh_opts[@]}" "$target" "$@"; }
# Runs a bash script on the host with a terminal, so sudo can ask for a password.
remote() {
  local script="$1"; shift
  ssh -t "${ssh_opts[@]}" "$target" "bash -c $(printf '%q' "$script") loams-install $(printf '%q ' "$@")"
}
cleanup() { ssh "${ssh_opts[@]}" -O exit "$target" >/dev/null 2>&1 || true; }
trap cleanup EXIT

say "connecting to $target"
ssh_cmd true

say "checking Docker on $target"
# shellcheck disable=SC2016 # expanded on the host
remote '
set -euo pipefail
sudo=""; [ "$(id -u)" -eq 0 ] || sudo="sudo"
if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is missing: installing it with https://get.docker.com"
  command -v curl >/dev/null 2>&1 || { $sudo apt-get update && $sudo apt-get install -y curl; } \
    || { echo "install curl first" >&2; exit 1; }
  curl -fsSL https://get.docker.com | $sudo sh
  $sudo systemctl enable --now docker 2>/dev/null || true
fi
docker_cmd="docker"; docker info >/dev/null 2>&1 || docker_cmd="$sudo docker"
$docker_cmd compose version >/dev/null 2>&1 \
  || { echo "docker compose (v2) is missing; install the docker-compose-plugin package" >&2; exit 1; }
echo "Docker: $($docker_cmd --version)"
'

say "copying the compose bundle to $target:$dir"
tar -C "$deploy" -czf - \
  --exclude='release/.env' --exclude='release/secrets/api-keys' --exclude='release/secrets/engine.env' \
  --exclude='release/build.sh' --exclude='release/Containerfile.*' \
  release loams-postgres-dev \
  | ssh_cmd "mkdir -p $(printf '%q' "$dir") && tar -xzf - -C $(printf '%q' "$dir")"

if ssh_cmd "test -f $(printf '%q' "$dir")/release/.env" && ((!reconfigure)); then
  say "keeping the existing .env (pass --reconfigure to rewrite it)"
else
  [[ -n "$headscale" ]] && login_server="$headscale"
  if [[ -n "$login_server" ]]; then
    say "warning: Headscale is coming soon and unsupported in 0.1.0-preview.2; the hosted console cannot reach an engine served over its http"
  fi
  if [[ -z "$authkey" && -z "$headscale" && $assume_yes -eq 0 ]]; then
    read -r -s -p "Tailnet auth key (TS_AUTHKEY; empty to log in from the sidecar's log): " authkey
    echo
  fi
  serve_mode="https"; [[ -n "$login_server" ]] && serve_mode="http"

  envfile="$(mktemp)"
  trap 'rm -f "$envfile"; cleanup' EXIT
  chmod 600 "$envfile"
  cp "$release/.env.example" "$envfile"
  setkv() {
    local tmp; tmp="$(mktemp)"
    awk -v k="$1" -v v="$2" 'BEGIN{FS="="} $1==k{print k "=" v; f=1; next} {print} END{if(!f) print k "=" v}' \
      "$envfile" >"$tmp"
    cat "$tmp" >"$envfile"; rm -f "$tmp"
  }
  setkv LOAMS_VERSION "$version"
  setkv TS_AUTHKEY "$authkey"
  setkv TS_LOGIN_SERVER "$login_server"
  setkv TS_SERVE_MODE "$serve_mode"
  if [[ -n "$headscale" ]]; then
    setkv HEADSCALE_SERVER_URL "$headscale"
    setkv HEADSCALE_BIND "0.0.0.0"
  fi
  if ((!all)); then
    setkv LOAMS_POSTGRES_COMPOSE "./postgres-off.yaml"
  fi
  say "writing $dir/release/.env"
  ssh_cmd "umask 077 && cat > $(printf '%q' "$dir")/release/.env" <"$envfile"
fi
ssh_cmd "umask 077 && touch $(printf '%q' "$dir")/release/secrets/api-keys"

((all)) && services=""
say "starting the stack"
# shellcheck disable=SC2016 # expanded on the host
remote '
set -euo pipefail
cd "$1/release"
services="$2"; headscale="$3"
sudo=""; [ "$(id -u)" -eq 0 ] || sudo="sudo"
dc="docker compose"; docker info >/dev/null 2>&1 || dc="$sudo docker compose"
if [ -n "$headscale" ]; then
  $dc --profile headscale up -d headscale
  # A minted key lives an hour, so mint a fresh one on every run until the
  # sidecar has joined (its state volume then keeps it logged in).
  if ! $dc exec -T tailscale tailscale status --json 2>/dev/null | tr -d "\n " | grep -q "\"BackendState\":\"Running\""; then
    for _ in $(seq 30); do $dc exec -T headscale headscale users list >/dev/null 2>&1 && break; sleep 1; done
    $dc exec -T headscale headscale users create loams >/dev/null 2>&1 || true
    uid="$($dc exec -T headscale headscale users list -o json | tr -d "\n\t " | sed -n "s/.*\"id\":\([0-9]*\),\"name\":\"loams\".*/\1/p")"
    [ -n "$uid" ] || { echo "could not find the headscale user loams" >&2; exit 1; }
    key="$($dc exec -T headscale headscale preauthkeys create --user "$uid" --reusable --expiration 1h | tail -n 1 | tr -d "\r")"
    sed -i "s|^TS_AUTHKEY=.*|TS_AUTHKEY=$key|" .env
    echo "minted a Headscale pre-auth key for user loams"
  fi
fi
$dc pull --quiet --ignore-pull-failures $services
$dc up -d $services
echo "waiting for the engine"
for _ in $(seq 60); do
  if $dc exec -T loams curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1; then ok=1; break; fi
  sleep 2
done
[ "${ok:-0}" = 1 ] || { echo "the engine is not healthy yet: $dc logs loams" >&2; exit 1; }
name="$($dc exec -T tailscale tailscale status --json 2>/dev/null | tr -d "\n" | sed -n "s/.*\"Self\":{[^}]*\"DNSName\":\"\([^\"]*\)\".*/\1/p" | sed "s/\.$//")"
mode="$(sed -n "s/^TS_SERVE_MODE=//p" .env)"
if [ -n "$name" ]; then
  echo "Engine on your tailnet: ${mode:-https}://$name  (LOAMS_ENGINE_URL for the console)"
else
  echo "The sidecar has not joined a tailnet yet: $dc logs tailscale"
fi
' "$dir" "$services" "$headscale"
say "done"
