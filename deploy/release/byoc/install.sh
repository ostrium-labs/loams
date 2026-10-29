#!/usr/bin/env bash
# Installs the Loams stack on a machine of your own (BYOC) over SSH:
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
#   --login-server URL      Headscale URL (coming soon, unsupported in 0.1.1);
#                           [default: $TS_LOGIN_SERVER, else empty = Tailscale]
#   --headscale URL         (coming soon, unsupported in 0.1.1) also run Headscale on the host (`--profile
#                           headscale`) at this public URL, mint the auth key
#                           there, and serve the engine over http
#   --version V             image tag [default: 0.1.3]
#   --tikv                  add the `tikv` profile: PD, TiKV and TiDB (Loams
#                           SQL; the metastore and Live on TiKV). Several GB
#                           more of disk and RAM
#   --rustfs                add the `rustfs` profile: Loams Postgres's storage
#                           in an S3 bucket (RustFS) instead of local folders
#   --all                   both: --tikv --rustfs
#   --profiles "A,B"        add these profiles too (house-tailnet, postgres-control, ...)
#   --no-postgres           leave the Loams Postgres stack out (postgres-off.yaml)
#   --services "A B"        start only these services (and what they depend on)
#                           [default: every service of the stack's profiles]
#   (Profiles and --no-postgres go into a new .env: pick them on the first
#   install, since a stack's data stays on the stores it started on.)
#   --cloud aws|gcp|azure   the host is a VM in that cloud with an identity (instance
#                           role, service account, managed identity): the tailnet key
#                           and setup token come from the cloud's secret store
#                           (deploy/byoc/lib/loams-byoc-secrets), not the prompt, and
#                           the engine reaches the bucket with the VM's identity (no key)
#   --bucket URL            with --cloud: s3://bucket/prefix, gs://bucket/prefix or
#                           az://container/prefix (required)
#   --region R              aws: the bucket's and secrets' region (required)
#   --project P             gcp: the project holding the secrets [default: the VM's]
#   --storage-account A     azure: the storage account (required)
#   --vault V               azure: the Key Vault holding the secrets (required)
#   --client-id ID          azure: the user-assigned identity's client ID
#   --secret-prefix P       the secrets' prefix [default: loams/<dir> on aws,
#                           loams-<dir> elsewhere]
#   --kafka K               the Kafka endpoint for your apps and Loams' connectors:
#                           none, iggy (the `kafka` profile: Kafka clients on the VM),
#                           confluent (the `connectors` profile: `loams-fabric connect`
#                           to and from your Confluent Cloud cluster), or iggy,confluent
#                           [default: none]; docs/guides/kafka.md compares them
#   --confluent-bootstrap B with confluent: the cluster's bootstrap endpoint, host:port
#                           (the confluent module's bootstrap_endpoint; required)
#   --confluent-integration I
#                           with confluent: the integration whose key the VM fetches,
#                           secrets/connect/0/confluent-<I>/ [default: confluent]
#   --confluent-topics "T[=S],..."
#                           with confluent: a source instance per topic T, into the
#                           Loams stream S [default S: T]
#   --confluent-sinks "S=T,..."
#                           with confluent: a sink instance per Loams stream S, to topic T
#                           (at least one of --confluent-topics and --confluent-sinks)
#   --confluent-prefix P    with confluent: consumer group and transactional id prefix,
#                           the module's consumer_group_prefix [default: loams-<dir>-]
#   --confluent-rate N      with confluent: each instance's limit in bytes a second
#                           [default: 65536, the example's bound, D1828]
#   --render-env-only       print the .env and secrets/engine.env that --cloud would
#                           write (secret values as <from secret store>), and with
#                           --kafka confluent the instance files; connect to nothing
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
version="0.1.3"
profiles=""
postgres=1
services=""
reconfigure=0
assume_yes=0
target=""
cloud=""
bucket=""
region=""
project=""
storage_account=""
vault=""
client_id=""
secret_prefix=""
render_only=0
kafka="none"
cf_bootstrap=""
cf_integration=""
cf_topics=""
cf_sinks=""
cf_prefix=""
cf_rate=""

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
    --tikv) profiles="$profiles,tikv"; shift ;;
    --rustfs) profiles="$profiles,rustfs"; shift ;;
    --all) profiles="$profiles,tikv,rustfs"; shift ;;
    --profiles) profiles="$profiles,${2:?}"; shift 2 ;;
    --no-postgres) postgres=0; shift ;;
    --services) services="${2:?}"; shift 2 ;;
    --cloud) cloud="${2:?}"; shift 2 ;;
    --bucket) bucket="${2:?}"; shift 2 ;;
    --region) region="${2:?}"; shift 2 ;;
    --project) project="${2:?}"; shift 2 ;;
    --storage-account) storage_account="${2:?}"; shift 2 ;;
    --vault) vault="${2:?}"; shift 2 ;;
    --client-id) client_id="${2-}"; shift 2 ;;
    --secret-prefix) secret_prefix="${2:?}"; shift 2 ;;
    --render-env-only) render_only=1; shift ;;
    --kafka) kafka="${2:?}"; shift 2 ;;
    --confluent-bootstrap) cf_bootstrap="${2:?}"; shift 2 ;;
    --confluent-integration) cf_integration="${2:?}"; shift 2 ;;
    --confluent-topics) cf_topics="${2:?}"; shift 2 ;;
    --confluent-sinks) cf_sinks="${2:?}"; shift 2 ;;
    --confluent-prefix) cf_prefix="${2:?}"; shift 2 ;;
    --confluent-rate) cf_rate="${2:?}"; shift 2 ;;
    --reconfigure) reconfigure=1; shift ;;
    -y|--yes) assume_yes=1; shift ;;
    -h|--help) sed -n '2,72p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) die "unknown option: $1" ;;
    *) [[ -z "$target" ]] || die "one target only"; target="$1"; shift ;;
  esac
done
((render_only)) || [[ -n "$target" ]] || die "usage: install.sh [options] user@host (see --help)"
[[ "$port" =~ ^[0-9]+$ ]] || die "--port must be a number"
[[ "$dir" =~ ^[A-Za-z0-9._/-]+$ ]] || die "--dir may hold only letters, digits and ._/-"
[[ "$dir" != *..* && "$dir" != "." && "$dir" != "./" && "${dir%/}" != "" ]] || die "--dir $dir is not an install directory"
[[ "$services" =~ ^[a-z0-9_\ -]*$ ]] || die "--services takes service names"
profiles="${profiles#,}"
[[ "$profiles" =~ ^[a-z0-9,-]*$ ]] || die "--profiles takes profile names, comma-separated"
[[ "$version" =~ ^[A-Za-z0-9._-]+$ ]] || die "--version is not an image tag"
[[ -f "$release/compose.yaml" && -f "$deploy/loams-postgres-dev/compose.yaml" ]] \
  || die "run from a Loams checkout: $release/compose.yaml or the Postgres stack is missing"

# --- --kafka: the Kafka endpoint (CC1 Task 8, D1820) ------------------------------
# Iggy and Confluent are each named: there is no "both" or "all", so a second
# endpoint (and its bill) is never turned on by a shorthand.
use_iggy=0; use_confluent=0
case "$kafka" in
  none) ;;
  iggy) use_iggy=1 ;;
  confluent) use_confluent=1 ;;
  iggy,confluent|confluent,iggy) use_iggy=1; use_confluent=1 ;;
  *) die "--kafka is none, iggy, confluent or iggy,confluent (each named)" ;;
esac
((use_iggy)) && profiles="${profiles:+$profiles,}kafka"
cf_dir_name="${dir##*/}"; cf_dir_name="${cf_dir_name:-loams}"
cf_instdir=""
if ((use_confluent)); then
  profiles="${profiles:+$profiles,}connectors"
  [[ -n "$cf_bootstrap" ]] || die "--kafka confluent needs --confluent-bootstrap (the module's bootstrap_endpoint)"
  # Confluent's bootstrap_endpoint sometimes carries SASL_SSL:// (CC0 R0.12).
  cf_bootstrap="$(sed -E 's#(^|,)[[:space:]]*[A-Za-z_]+://#\1#g; s/[[:space:]]//g' <<<"$cf_bootstrap")"
  [[ "$cf_bootstrap" =~ ^[A-Za-z0-9.-]+:[0-9]{1,5}(,[A-Za-z0-9.-]+:[0-9]{1,5})*$ ]] \
    || die "--confluent-bootstrap is host:port (comma separated)"
  cf_integration="${cf_integration:-confluent}"
  [[ "$cf_integration" =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] \
    || die "--confluent-integration is lowercase letters, digits and -, at most 31"
  cf_prefix="${cf_prefix:-loams-$cf_dir_name-}"
  [[ "$cf_prefix" =~ ^[A-Za-z0-9._-]{1,200}$ ]] || die "--confluent-prefix may hold only letters, digits and ._-"
  cf_rate="${cf_rate:-65536}"
  if ! [[ "$cf_rate" =~ ^[1-9][0-9]{3,11}$ ]] || ((cf_rate < 1024)); then
    die "--confluent-rate is bytes a second, at least 1024"
  fi
  [[ -n "$cf_topics$cf_sinks" ]] || die "--kafka confluent needs --confluent-topics or --confluent-sinks"
  cf_instdir="$(mktemp -d)"
  trap 'rm -rf "$cf_instdir"' EXIT   # cleanup (below) takes it over
  # An instance name: [a-z0-9-], at most 63, from the integration and the topic.
  cf_name() {
    local t pre="cf-$cf_integration-" sfx="-$2"
    t="$(tr 'A-Z._' 'a-z--' <<<"$1" | tr -cs '\na-z0-9-' '-' | sed -E 's/^-+//; s/-+$//')"
    t="${t:0:$((63 - ${#pre} - ${#sfx}))}"; t="${t%-}"
    echo "$pre${t:-x}$sfx"
  }
  cf_render() { # <template> <name> <topic> <stream>
    local out="$cf_instdir/$2.yaml"
    [[ ! -e "$out" ]] || die "two connector instances would be named $2: rename a topic or stream"
    sed -e "s|@NAME@|$2|g" -e "s|@TOPIC@|$3|g" -e "s|@STREAM@|$4|g" \
      -e "s|@BOOTSTRAP@|$cf_bootstrap|g" -e "s|@INTEGRATION@|$cf_integration|g" \
      -e "s|@PREFIX@|$cf_prefix|g" -e "s|@RATE@|$cf_rate|g" "$release/connect/templates/$1" >"$out"
    chmod 0644 "$out"   # connect reads it as uid 10001
  }
  name_ok='^[A-Za-z0-9._-]{1,249}$'
  IFS=',' read -r -a cf_list <<<"$cf_topics"
  for e in "${cf_list[@]}"; do
    [[ -n "$e" ]] || continue
    topic="${e%%=*}"; stream="${e#*=}"
    [[ "$topic" =~ $name_ok && "$stream" =~ $name_ok ]] || die "--confluent-topics: '$e' is not T or T=S (letters, digits and ._-)"
    cf_render confluent-source.yaml "$(cf_name "$topic" in)" "$topic" "$stream"
  done
  IFS=',' read -r -a cf_list <<<"$cf_sinks"
  for e in "${cf_list[@]}"; do
    [[ -n "$e" ]] || continue
    [[ "$e" == *=* ]] || die "--confluent-sinks: '$e' is not S=T"
    stream="${e%%=*}"; topic="${e#*=}"
    [[ "$topic" =~ $name_ok && "$stream" =~ $name_ok ]] || die "--confluent-sinks: '$e' is not S=T (letters, digits and ._-)"
    cf_render confluent-sink.yaml "$(cf_name "$topic" out)" "$topic" "$stream"
  done
elif [[ -n "$cf_bootstrap$cf_integration$cf_topics$cf_sinks$cf_prefix$cf_rate" ]]; then
  die "the --confluent-* options go with --kafka confluent (or iggy,confluent)"
fi

# --- --cloud: the bucket and identity settings, checked ---------------------------
engine_env_lines=""
if [[ -n "$cloud" ]]; then
  [[ "$cloud" =~ ^(aws|gcp|azure)$ ]] || die "--cloud is aws, gcp or azure"
  [[ -n "$bucket" ]] || die "--cloud needs --bucket (s3://, gs:// or az:// URL)"
  case "$cloud:$bucket" in
    aws:s3://?*|gcp:gs://?*|azure:az://?*) ;;
    *) die "--bucket $bucket does not match --cloud $cloud (s3://, gs:// or az://)" ;;
  esac
  [[ "$bucket" =~ ^(s3|gs|az)://[A-Za-z0-9._-]+(/[A-Za-z0-9._/-]*)?$ ]] || die "--bucket is not a plain bucket URL"
  dir_name="${dir##*/}"; dir_name="${dir_name:-loams}"
  if [[ -z "$secret_prefix" ]]; then
    case "$cloud" in aws) secret_prefix="loams/$dir_name" ;; *) secret_prefix="loams-$dir_name" ;; esac
  fi
  [[ "$secret_prefix" =~ ^[A-Za-z0-9][A-Za-z0-9/_-]{0,100}$ ]] || die "--secret-prefix is not valid"
  case "$cloud" in
    aws)
      [[ "$region" =~ ^[a-z]{2}(-[a-z]+)+-[0-9]$ ]] || die "--cloud aws needs --region (for example us-east-1)"
      engine_env_lines="AWS_REGION=$region" ;;
    gcp)
      [[ -z "$project" || "$project" =~ ^[a-z][a-z0-9-]{4,28}[a-z0-9]$ ]] || die "--project is not a project ID" ;;
    azure)
      [[ "$storage_account" =~ ^[a-z0-9]{3,24}$ ]] || die "--cloud azure needs --storage-account"
      [[ "$vault" =~ ^[A-Za-z0-9][A-Za-z0-9-]{1,22}[A-Za-z0-9]$ ]] || die "--cloud azure needs --vault"
      [[ -z "$client_id" || "$client_id" =~ ^[0-9a-fA-F-]{36}$ ]] || die "--client-id is not a GUID"
      engine_env_lines="AZURE_STORAGE_ACCOUNT_NAME=$storage_account"$'\n'"AZURE_CREDENTIAL_TYPE=managed_identity"
      [[ -z "$client_id" ]] || engine_env_lines="$engine_env_lines"$'\n'"AZURE_STORAGE_CLIENT_ID=$client_id" ;;
  esac
elif [[ -n "$bucket$region$project$storage_account$vault$client_id$secret_prefix" ]]; then
  die "--bucket, --region, --project, --storage-account, --vault, --client-id and --secret-prefix go with --cloud"
fi

# The connect socket takes API keys only (CC1 ruling R1.1), so the connectors
# profile needs apikey in the engine's auth mode beside the tailnet's.
auth_mode=""
((use_confluent)) && auth_mode="tailscale,apikey"

if ((render_only)); then
  [[ -n "$cloud" || "$kafka" != none ]] || die "--render-env-only needs --cloud or --kafka"
  if [[ -n "$cloud" ]]; then
    echo "# .env (keys that --cloud sets)"
  else
    echo "# .env (keys that --kafka sets)"
  fi
  echo "LOAMS_VERSION=$version"
  [[ -z "$cloud" ]] || echo "LOAMS_BUCKET=$bucket"
  echo "COMPOSE_PROFILES=$profiles"
  [[ -z "$auth_mode" ]] || echo "LOAMS_AUTH_MODE=$auth_mode"
  if [[ -n "$cloud" ]]; then
    echo "TS_AUTHKEY=<from secret store>"
    echo "LOAMS_SETUP_TOKEN=<from secret store>"
    echo "# secrets/engine.env"
    [[ -z "$engine_env_lines" ]] || echo "$engine_env_lines"
    echo "# secrets: $secret_prefix (tailnet-authkey, setup-token, api-keys optional) in the $cloud secret store"
    if ((use_confluent)); then
      echo "# /etc/loams-byoc/config.env (Confluent)"
      echo "CONNECT_NAMESPACE=0"
      echo "CONFLUENT_INTEGRATIONS=$cf_integration"
      echo "# secrets: $secret_prefix/confluent-$cf_integration-{api-key,api-secret} (loams byoc confluent key create)"
    fi
  fi
  if ((use_confluent)); then
    echo "# secrets/connect/engine-key and connect-admin-token: made on the host at start"
    for f in "$cf_instdir"/*.yaml; do
      echo "# connect/instances/${f##*/}"
      cat "$f"
    done
  fi
  exit 0
fi

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
cleanup() {
  [[ -z "$cf_instdir" ]] || rm -rf "$cf_instdir"
  ssh "${ssh_opts[@]}" -O exit "$target" >/dev/null 2>&1 || true
}
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
  --exclude='release/secrets/connect' --exclude='release/connect/instances/*.yaml' \
  release loams-postgres-dev \
  | ssh_cmd "mkdir -p $(printf '%q' "$dir") && tar -xzf - -C $(printf '%q' "$dir")"

if ssh_cmd "test -f $(printf '%q' "$dir")/release/.env" && ((!reconfigure)); then
  say "keeping the existing .env (pass --reconfigure to rewrite it)"
  if [[ -n "$profiles" ]] || ((!postgres)); then
    say "warning: --tikv, --rustfs, --all, --profiles, --kafka and --no-postgres only go into a new .env; the existing one's COMPOSE_PROFILES stays"
  fi
else
  [[ -n "$headscale" ]] && login_server="$headscale"
  if [[ -n "$login_server" ]]; then
    say "warning: Headscale is coming soon and unsupported in 0.1.1; the hosted console cannot reach an engine served over its http"
  fi
  if [[ -z "$cloud" && -z "$authkey" && -z "$headscale" && $assume_yes -eq 0 ]]; then
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
  # With --cloud both come from the cloud's secret store, on the host, below.
  [[ -n "$cloud" ]] || setkv TS_AUTHKEY "$authkey"
  [[ -z "$cloud" ]] || setkv LOAMS_BUCKET "$bucket"
  setkv TS_LOGIN_SERVER "$login_server"
  setkv TS_SERVE_MODE "$serve_mode"
  # First-run setup with auth on needs a setup token (docs/release/preview.md,
  # "First-run setup"). It stays in the host's .env, mode 0600, and is not printed.
  [[ -n "$cloud" ]] || setkv LOAMS_SETUP_TOKEN "$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  # The `kafka` profile refuses .env.example's placeholder for Iggy's root
  # password (config/kafka-guard.sh), so a new .env gets one of its own,
  # whichever profiles it starts with. Also never printed.
  setkv KAFKA_IGGY_PASSWORD "$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"
  if [[ -n "$headscale" ]]; then
    setkv HEADSCALE_SERVER_URL "$headscale"
    setkv HEADSCALE_BIND "0.0.0.0"
  fi
  setkv COMPOSE_PROFILES "$profiles"
  [[ -z "$auth_mode" ]] || setkv LOAMS_AUTH_MODE "$auth_mode"
  if ((!postgres)); then
    setkv LOAMS_POSTGRES_COMPOSE "./postgres-off.yaml"
  fi
  say "writing $dir/release/.env"
  ssh_cmd "umask 077 && cat > $(printf '%q' "$dir")/release/.env" <"$envfile"
fi
# An empty key file, owner-only until the start step gives it to the engine's
# group.
ssh_cmd "umask 077 && touch $(printf '%q' "$dir")/release/secrets/api-keys"

if ((use_confluent)); then
  # Instance files the host already has are kept (the user may have edited
  # them) unless --reconfigure.
  keep="--skip-old-files"; ((reconfigure)) && keep="--overwrite"
  say "writing $dir/release/connect/instances ($(find "$cf_instdir" -name '*.yaml' | wc -l | tr -d ' ') Confluent instances)"
  # The files only, not the directory: its mode stays the bundle's.
  (cd "$cf_instdir" && tar -czf - -- *.yaml) \
    | ssh_cmd "mkdir -p $(printf '%q' "$dir")/release/connect/instances && tar -xzf - $keep --no-same-owner -C $(printf '%q' "$dir")/release/connect/instances"
  if [[ -z "$cloud" ]]; then
    say "put the Confluent API key's id and secret in $dir/release/secrets/connect/0/confluent-$cf_integration/username and password (mode 0600, owner uid 10001): without --cloud nothing fetches them"
  fi
fi

if [[ -n "$cloud" ]]; then
  say "setting up the $cloud identity and secrets on $target"
  # The engine's own settings for the bucket: no key, the VM's identity is used.
  printf '%s\n' "$engine_env_lines" \
    | ssh_cmd "umask 077 && cat > $(printf '%q' "$dir")/release/secrets/engine.env"
  cfg="$(mktemp)"
  {
    echo "LOAMS_BYOC_CLOUD=$cloud"
    echo "SECRET_PREFIX=$secret_prefix"
    echo "REGION=$region"
    echo "GCP_PROJECT=$project"
    echo "AZURE_VAULT=$vault"
    echo "AZURE_CLIENT_ID=$client_id"
    # The Confluent key's secrets -> secrets/connect/0/confluent-<i>/ (D1827).
    if ((use_confluent)); then
      echo "CONNECT_NAMESPACE=0"
      echo "CONFLUENT_INTEGRATIONS=$cf_integration"
    fi
  } >"$cfg"
  ssh_cmd "cat > loams-byoc-config.tmp" <"$cfg"
  ssh_cmd "cat > loams-byoc-secrets.tmp" <"$deploy/byoc/lib/loams-byoc-secrets"
  ssh_cmd "cat > loams-byoc.service.tmp" <"$deploy/byoc/lib/loams-byoc.service"
  ssh_cmd "cat > loams-byoc-confluent.service.tmp" <"$deploy/byoc/lib/loams-byoc-confluent.service"
  ssh_cmd "cat > loams-byoc-confluent.timer.tmp" <"$deploy/byoc/lib/loams-byoc-confluent.timer"
  rm -f "$cfg"
  # shellcheck disable=SC2016 # expanded on the host
  remote '
set -euo pipefail
dir="$1"; sudo=""; [ "$(id -u)" -eq 0 ] || sudo="sudo"
case "$dir" in /*) abs="$dir" ;; *) abs="$HOME/$dir" ;; esac
command -v jq >/dev/null 2>&1 || { $sudo apt-get update -qq && $sudo apt-get install -y -qq jq curl; }
$sudo install -d -m 755 /etc/loams-byoc
{ cat loams-byoc-config.tmp; echo "LOAMS_BYOC_STACK=$abs/release"; echo "LOAMS_BYOC_OWNER=$(id -un)"; } | $sudo tee /etc/loams-byoc/config.env >/dev/null
$sudo install -m 755 loams-byoc-secrets.tmp /usr/local/bin/loams-byoc-secrets
sed "s#^WorkingDirectory=.*#WorkingDirectory=$abs/release#" loams-byoc.service.tmp | $sudo tee /etc/systemd/system/loams-byoc.service >/dev/null
$sudo install -m 644 loams-byoc-confluent.service.tmp /etc/systemd/system/loams-byoc-confluent.service
$sudo install -m 644 loams-byoc-confluent.timer.tmp /etc/systemd/system/loams-byoc-confluent.timer
rm -f loams-byoc-config.tmp loams-byoc-secrets.tmp loams-byoc.service.tmp \
  loams-byoc-confluent.service.tmp loams-byoc-confluent.timer.tmp
$sudo systemctl daemon-reload
$sudo systemctl enable loams-byoc.service   # at boot: fetch the secrets, then compose up
# every 5 minutes: rotated Confluent keys into the connect role files (no-op without integrations)
$sudo systemctl enable --now loams-byoc-confluent.timer
# The first fetch happens here, so the first start already has the tailnet key.
$sudo /usr/local/bin/loams-byoc-secrets
' "$dir"
fi

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
# The engine (uid and gid 10001) reads secrets/api-keys once LOAMS_AUTH_MODE
# takes API keys: the file goes to its group, read-only (0640), and stays the
# SSH user (who appends keys to it); README.md, API keys. chgrp runs as root
# in the engine image, so that gid 10001 is the engine gid under rootless
# Docker too.
docker="docker"; docker info >/dev/null 2>&1 || docker="$sudo docker"
image="$($dc config --images 2>/dev/null | grep -m1 "/loams:" || true)"
if [ -z "$image" ] \
  || ! $docker run --rm --user 0:0 --network none --cap-drop ALL --cap-add CHOWN \
         --security-opt no-new-privileges:true --entrypoint chgrp \
         -v "$PWD/secrets/api-keys:/run/api-keys" "$image" 10001 /run/api-keys \
  || ! chmod 0640 secrets/api-keys; then
  echo "warning: the engine (gid 10001) cannot read secrets/api-keys yet; see API keys in README.md before turning API keys on" >&2
fi
# The connectors profile (CC1 Task 8): connect reaches the engine with its own
# API key, secrets/connect/engine-key, and guards its admin API with
# secrets/connect/connect-admin-token. Both are made once, owner uid 10001 and
# mode 0600 like the secret fetcher files (as root in the engine image, so
# that uid 10001 is the containers one under rootless Docker too); the key
# hash goes to secrets/api-keys. Neither value is printed.
if grep -Eq "^COMPOSE_PROFILES=([^#]*,)?connectors(,|[[:space:]]*$)" .env; then
  mkdir -p secrets/connect
  if [ -z "$image" ]; then
    echo "warning: no engine image: make secrets/connect/engine-key by hand (docs/guides/connectors/connect-role.md)" >&2
  else
    c=(--rm -i --user 0:0 --network none --cap-drop ALL --cap-add CHOWN --cap-add DAC_OVERRIDE
       --cap-add FOWNER --security-opt no-new-privileges:true --entrypoint sh
       -v "$PWD/secrets/connect:/c" "$image")
    if [ "$($docker run "${c[@]}" -c "test -s /c/engine-key && echo have" </dev/null)" != have ]; then
      errf="$(mktemp)"
      if key="$($docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges:true \
                 "$image" auth new-key --label connect 2>"$errf")" && [ -n "$key" ] \
         && line="$(grep -E "^[^ ]+ [\$]argon2id" "$errf" | tail -n 1)" && [ -n "$line" ]; then
        printf "%s\n" "$line" >>secrets/api-keys
        printf "%s" "$key" | $docker run "${c[@]}" -c "umask 077 && cat >/c/engine-key && chown 10001:10001 /c/engine-key"
        echo "made the connect role API key: secrets/connect/engine-key, its hash in secrets/api-keys"
      else
        echo "warning: could not make the connect role API key:" >&2; cat "$errf" >&2
      fi
      key=""; rm -f "$errf"
    fi
    $docker run "${c[@]}" -c "umask 077; test -s /c/connect-admin-token || od -An -N32 -tx1 /dev/urandom | tr -d \" \\n\" >/c/connect-admin-token; chown 10001:10001 /c /c/connect-admin-token && chmod 0700 /c" </dev/null \
      || echo "warning: could not make secrets/connect/connect-admin-token" >&2
  fi
fi
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
  echo "First-run setup: the console asks for LOAMS_SETUP_TOKEN, in $1/release/.env on this host"
else
  echo "The sidecar has not joined a tailnet yet: $dc logs tailscale"
fi
' "$dir" "$services" "$headscale"
say "done"
