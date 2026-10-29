# shellcheck shell=bash
# Confluent Cloud API helper for the CFB benchmark (bench/confluent).
#
#   source bench/confluent/lib/cc.sh
#   cc GET  "/cmk/v2/clusters?environment=$CFB_ENV"
#   cc POST /cmk/v2/clusters "$json"
#
# Credentials: the Cloud key CONFLUENT_API_KEY / _SECRET from $CFB_CLOUD_ENV_FILE
# (default ~/confluent.env), read in a subshell and handed to curl on stdin, so
# they never appear in argv, in the environment of the caller or on screen.
#
# Safety (owner rules, 2026-10-28):
#   - only environment env-3kqygo (loams-ci) is ever changed;
#   - the Kafka oracle (env-3kqddw, lkc-120zwvj, sa-3n32p6j) is refused outright;
#   - DELETE is allowed only for ids this benchmark created (state/created.tsv).
CFB_ENV="${CFB_ENV:-env-3kqygo}"
CFB_API="${CFB_API:-https://api.confluent.cloud}"
CFB_STATE="${CFB_STATE:-$HOME/.cfb-state}"
CFB_CLOUD_ENV_FILE="${CFB_CLOUD_ENV_FILE:-$HOME/confluent.env}"
CFB_PROTECTED=" env-3kqddw lkc-120zwvj sa-3n32p6j GSSXLGAUL3DYLNPT "
mkdir -p "$CFB_STATE"; chmod 700 "$CFB_STATE"
[[ "$CFB_ENV" == env-3kqygo ]] || { echo "cc.sh: CFB_ENV must be env-3kqygo" >&2; return 1 2>/dev/null || exit 1; }

cc_refuse() {  # text -> nonzero if it names a protected resource
  local p
  for p in $CFB_PROTECTED; do
    [[ "$1" != *"$p"* ]] || { echo "cc.sh: refusing, $p is protected" >&2; return 1; }
  done
}

# cc METHOD PATH-or-URL [json] -> body on stdout; HTTP code in $CFB_STATE/last.status
cc() {
  local m="$1" p="$2" url
  cc_refuse "$p ${3:-}" || return 1
  if [[ "$m" == DELETE ]]; then
    local id; id="$(sed -E 's#.*/([^/?]+)(\?.*)?$#\1#' <<<"$p")"
    grep -q -P "\t$id\t" "$CFB_STATE/created.tsv" 2>/dev/null \
      || { echo "cc.sh: refusing DELETE of $id: not created by this benchmark" >&2; return 1; }
  fi
  if [[ "$p" == /cmk/* && "$m" != GET && "$p" == *environment=* && "$p" != *"environment=$CFB_ENV"* ]]; then
    echo "cc.sh: refusing $m outside $CFB_ENV" >&2; return 1
  fi
  [[ "$p" == http* ]] && url="$p" || url="$CFB_API$p"
  local args=(-sS -X "$m" -H 'Content-Type: application/json' --max-time 120 --retry 2 -o "$CFB_STATE/last.$$.body" -w '%{http_code}')
  [[ $# -lt 3 ]] || args+=(--data "$3")
  (
    set +x
    # shellcheck disable=SC1090
    set -a; . "$CFB_CLOUD_ENV_FILE"; set +a
    # ~/confluent.env: the owner's Cloud key is CONFLUENT_API_KEY/_SECRET (the
    # CONFLUENT_CLOUD_* entries there are placeholders, 2026-10-28).
    printf 'user = "%s:%s"\n' "${CFB_CLOUD_KEY_ID:-$CONFLUENT_API_KEY}" "${CFB_CLOUD_KEY_SECRET:-$CONFLUENT_API_SECRET}"
  ) | curl -K - "${args[@]}" "$url" >"$CFB_STATE/last.$$.status" 2>"$CFB_STATE/last.$$.err" || echo 000 >"$CFB_STATE/last.$$.status"
  cat "$CFB_STATE/last.$$.body"
}
cc_status() { cat "$CFB_STATE/last.$$.status"; }

# Basic auth with a Kafka/Flink key file (id=..., secret=...) for data-plane REST.
cc_keyed() {  # keyfile METHOD URL [json]
  local kf="$1" m="$2" url="$3"
  cc_refuse "$url ${4:-}" || return 1
  local args=(-sS -X "$m" -H 'Content-Type: application/json' --max-time 120 -o "$CFB_STATE/last.$$.body" -w '%{http_code}')
  [[ $# -lt 4 ]] || args+=(--data "$4")
  printf 'user = "%s:%s"\n' "$(sed -n 's/^id=//p' "$kf")" "$(sed -n 's/^secret=//p' "$kf")" \
    | curl -K - "${args[@]}" "$url" >"$CFB_STATE/last.$$.status" 2>"$CFB_STATE/last.$$.err" || echo 000 >"$CFB_STATE/last.$$.status"
  cat "$CFB_STATE/last.$$.body"
}

# Record a created/deleted resource in our state and in the shared CI ledgers.
CFB_SHARED="${CFB_SHARED:-$HOME/Documents/Ostriumlabs/loams-wt/.he0-shared}"
cc_log_created() {  # kind id name where deadline_epoch note
  local now; now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$now" "$1" "$2" "$3" "$4" "$5" "${6:-}" >>"$CFB_STATE/created.tsv"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$now" "$1" "$2" "$3" "$CFB_ENV" "$4" cfb "deadline $(date -u -d "@$5" +%H:%MZ); ${6:-}" \
    >>"$CFB_SHARED/confluent-ci-created.tsv"
}
cc_log_deleted() {  # kind id name where note
  local now; now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '%s\t%s\t%s\t%s\t%s\n' "$now" "$1" "$2" "$3" "${5:-}" >>"$CFB_STATE/deleted.tsv"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$now" "$1" "$2" "$3" "$CFB_ENV" "$4" cfb "${5:-}" \
    >>"$CFB_SHARED/confluent-ci-deleted.tsv"
}
