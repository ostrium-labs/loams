#!/usr/bin/env bash
# CFB provisioning: create, resize and delete the benchmark's Confluent Cloud
# resources in env-3kqygo, with a budget check before every create.
#
#   cfb.sh cluster NAME CLOUD REGION TYPE MAX_ECKU TTL_HOURS   Basic|Standard cluster + owner Kafka key
#   cfb.sh ecku    NAME MAX_ECKU                               change max eCKU (updates the guard's rate)
#   cfb.sh pool    NAME CLOUD REGION MAX_CFU TTL_HOURS         Flink compute pool + Flink key
#   cfb.sh projected [EXTRA_USD]                               projected total if everything runs to its deadline
#   cfb.sh delete  NAME                                        delete one resource (and its keys)
#   cfb.sh list
#
# Prices: deploy/tofu/modules/confluent/confluent-tables.json (read 2026-10-26 from
# confluent.io/confluent-cloud/pricing; upper bound where the page gives a range),
# Flink $0.21/CFU-hour (docs.confluent.io Flink billing). Names: loams-cfb-*, never
# loams-ci-* (the CI reaper deletes those).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
# shellcheck source=lib/cc.sh
source "$here/lib/cc.sh"
tables="$repo/deploy/tofu/modules/confluent/confluent-tables.json"
cap="${CFB_PLAN_CAP_USD:-$(cat "$CFB_STATE/stop_usd" 2>/dev/null || echo 380)}"
live="$CFB_STATE/live.tsv"; touch "$live" "$CFB_STATE/billed_done.tsv"
# One writer at a time for live.tsv (push.sh runs several of these at once, and
# the guard edits it too): a lock held for the whole command.
exec {lockfd}>"$CFB_STATE/live.lock"; flock -w 300 "$lockfd" || { echo "cfb.sh: live.tsv lock busy" >&2; exit 1; }
die() { echo "cfb.sh: $*" >&2; exit 1; }
now() { date +%s; }

price_ecku() { jq -r --arg t "$1" '.types[$t].ecku_hour_usd' "$tables"; }
free_ecku()  { jq -r --arg t "$1" '.types[$t].free_ecku // 0' "$tables"; }
rate_for() {  # type max_ecku|cku -> usd/h
  if [[ "$1" == dedicated ]]; then
    # Dedicated CKU-hours are not on Confluent's price page ("contact sales"); third-party
    # list estimates are ~$3.00-3.30. The guard uses $4.00 (CFB_DEDICATED_CKU_USD) until the
    # Costs API shows the real line.
    awk -v n="$2" -v p="${CFB_DEDICATED_CKU_USD:-4.00}" 'BEGIN{printf "%.4f", n*p}'; return
  fi
  awk -v p="$(price_ecku "$1")" -v f="$(free_ecku "$1")" -v n="$2" 'BEGIN{x=n-f; if(x<0)x=0; printf "%.4f", x*p}'
}

projected() {  # everything live billed to its deadline + done + data + extra
  awk -F'\t' -v data="$(cat "$CFB_STATE/data_usd" 2>/dev/null || echo 0)" -v extra="${1:-0}" '
    FILENAME==ARGV[1] { if ($1!="") { s=$6-$5; h=int((s+3599)/3600); t+=h*$7 }; next }
    { if ($1!="") { s=$2-$1; if (s<1) s=1; h=int((s+3599)/3600); t+=h*$3 } }
    END { printf "%.2f", t+data+extra }' "$live" "$CFB_STATE/billed_done.tsv"
}

budget_check() {  # new_usd
  local p; p="$(projected "$1")"
  awk -v p="$p" -v c="$cap" 'BEGIN{exit !(p<=c)}' || die "budget: projected \$$p would exceed the \$$cap plan cap; not creating"
  [[ ! -e "$CFB_STATE/ABORT" ]] || die "ABORT is set; not creating"
  echo "budget ok: projected \$$p of \$$cap"
}

live_add() {  # kind id name where created deadline rate
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$@" >>"$live"
}
live_get() { awk -F'\t' -v n="$1" '$3==n' "$live"; }

owner_id() {
  local kid; kid="$( (set -a; . "$CFB_CLOUD_ENV_FILE"; echo "${CFB_CLOUD_KEY_ID:-$CONFLUENT_API_KEY}") )"
  cc GET "/iam/v2/api-keys/$kid" | jq -r '.spec.owner.id'
}

mint_key() {  # resource_id name keyfile [environment]
  local body
  body="$(jq -cn --arg o "${CFB_KEY_OWNER:-$(owner_id)}" --arg r "$1" --arg e "$CFB_ENV" --arg n "$2" \
    '{spec:{display_name:$n, description:"CFB benchmark 2026-10-28 (bench/confluent); deleted at teardown", owner:{id:$o}, resource:{id:$r, environment:$e}}}')"
  local out; out="$(cc POST /iam/v2/api-keys "$body")"
  [[ "$(cc_status)" == 20* ]] || die "minting key for $1 answered HTTP $(cc_status): $(jq -c '.errors' <<<"$out")"
  (umask 077; jq -r '"id=" + .id + "\nsecret=" + .spec.secret' <<<"$out" >"$3")
  : >"$CFB_STATE/last.body"
  sed -n 's/^id=//p' "$3"
}

cmd_cluster() {
  local name="$1" cloud="$2" region="$3" type="$4" ecku="$5" ttl="$6"
  [[ "$name" == loams-cfb-* ]] || die "names must start with loams-cfb-"
  [[ -z "$(live_get "$name")" ]] || die "$name already exists"
  local rate; rate="$(rate_for "$type" "$ecku")"
  budget_check "$(awk -v r="$rate" -v h="$ttl" 'BEGIN{printf "%.2f", r*h}')"
  local kind; case "$type" in basic) kind=Basic ;; standard) kind=Standard ;; dedicated) kind=Dedicated ;; *) die "type $type not allowed" ;; esac
  local body
  if [[ "$kind" == Dedicated ]]; then  # ECKU is the CKU count here; public networking
    body="$(jq -cn --arg n "$name" --arg c "${cloud^^}" --arg r "$region" --arg e "$CFB_ENV" --argjson m "$ecku" \
      '{spec:{display_name:$n, availability:"SINGLE_ZONE", cloud:$c, region:$r, config:{kind:"Dedicated", cku:$m}, environment:{id:$e}}}')"
  else
    body="$(jq -cn --arg n "$name" --arg c "${cloud^^}" --arg r "$region" --arg k "$kind" --arg e "$CFB_ENV" --argjson m "$ecku" \
      '{spec:{display_name:$n, availability:"SINGLE_ZONE", cloud:$c, region:$r, config:{kind:$k, max_ecku:$m}, environment:{id:$e}}}')"
  fi
  local out; out="$(cc POST /cmk/v2/clusters "$body")"
  [[ "$(cc_status)" == 20* ]] || die "create $name: HTTP $(cc_status) $(jq -c '.errors' <<<"$out")"
  local id t0 deadline; id="$(jq -r .id <<<"$out")"; t0="$(now)"; deadline=$((t0 + ttl * 3600))
  live_add kafka-cluster "$id" "$name" "$cloud/$region" "$t0" "$deadline" "$rate"
  printf '%s\t%s\t%s\n' "$name" "$id" "$t0" >>"$CFB_STATE/clusters_all.tsv"  # data totals outlive live.tsv
  cc_log_created kafka-cluster "$id" "$name" "$cloud/$region" "$deadline" "$type max_ecku $ecku; CFB benchmark"
  echo "created $id $name ($type $cloud/$region max_ecku $ecku, \$$rate/h, deadline $(date -u -d "@$deadline" +%H:%MZ))"
  # release the live.tsv lock while waiting for provisioning (Dedicated takes up to hours)
  flock -u "$lockfd"
  local i st
  for i in $(seq 1 "$([[ "$kind" == Dedicated ]] && echo 1080 || echo 90)"); do
    out="$(cc GET "/cmk/v2/clusters/$id?environment=$CFB_ENV")"; st="$(jq -r '.status.phase // empty' <<<"$out")"
    [[ "$st" == PROVISIONED ]] && break; sleep 10
  done
  [[ "$st" == PROVISIONED ]] || die "$id not provisioned in time (phase $st)"
  local dir="$CFB_STATE/clusters/$name"; mkdir -p "$dir"; chmod 700 "$dir"
  jq '{id, name:.spec.display_name, cloud:.spec.cloud, region:.spec.region, kind:.spec.config.kind, max_ecku:.spec.config.max_ecku, cku:.spec.config.cku,
       bootstrap:(.spec.kafka_bootstrap_endpoint|sub("^SASL_SSL://";"")), rest:.spec.http_endpoint, created:.metadata.created_at}' <<<"$out" >"$dir/cluster.json"
  local kid; kid="$(mint_key "$id" "$name-owner" "$dir/key")"
  cc_log_created api-key "$kid" "$name-owner" "$cloud/$region" "$deadline" "owner key on $id (deleted with the cluster)"
  echo "$id ready; bootstrap $(jq -r .bootstrap "$dir/cluster.json"); key $kid"
}

cmd_ecku() {
  local name="$1" ecku="$2" line id type rate
  line="$(live_get "$name")"; [[ -n "$line" ]] || die "$name is not live"
  id="$(cut -f2 <<<"$line")"
  type="$(jq -r '.kind|ascii_downcase' "$CFB_STATE/clusters/$name/cluster.json")"
  rate="$(rate_for "$type" "$ecku")"
  # budget: the new rate for the rest of the cluster's life
  local old; old="$(cut -f7 <<<"$line")"
  local deadline created; created="$(cut -f5 <<<"$line")"; deadline="$(cut -f6 <<<"$line")"
  local h=$(( (deadline - created + 3599) / 3600 ))
  budget_check "$(awk -v n="$rate" -v o="$old" -v h="$h" 'BEGIN{d=(n-o)*h; if(d<0)d=0; printf "%.2f", d}')"
  local out; out="$(cc PATCH "/cmk/v2/clusters/$id" "$(jq -cn --arg e "$CFB_ENV" --argjson m "$ecku" '{spec:{config:{kind:"'"$(jq -r .kind "$CFB_STATE/clusters/$name/cluster.json")"'", max_ecku:$m}, environment:{id:$e}}}')")"
  [[ "$(cc_status)" == 20* ]] || die "PATCH max_ecku: HTTP $(cc_status) $(jq -c '.errors' <<<"$out")"
  if awk -v n="$rate" -v o="$old" 'BEGIN{exit !(n<o)}'; then
    # Lowering: bank the high-rate period as billed (whole hours, counted again
    # for the current hour: an over-count, the safe side), then restart the
    # live row at the lower rate from now.
    printf '%s\t%s\t%s\n' "$created" "$(now)" "$old" >>"$CFB_STATE/billed_done.tsv"
    awk -F'\t' -v OFS='\t' -v n="$name" -v r="$rate" -v t="$(now)" '$3==n{ $5=t; $7=r } {print}' "$live" >"$live.tmp" && mv "$live.tmp" "$live"
  else
    awk -F'\t' -v OFS='\t' -v n="$name" -v r="$rate" '$3==n{ $7=r } {print}' "$live" >"$live.tmp" && mv "$live.tmp" "$live"
  fi
  jq --argjson m "$ecku" '.max_ecku=$m' "$CFB_STATE/clusters/$name/cluster.json" >"$CFB_STATE/clusters/$name/c.tmp" && mv "$CFB_STATE/clusters/$name/c.tmp" "$CFB_STATE/clusters/$name/cluster.json"
  echo "$name max_ecku -> $ecku (guard rate \$$(live_get "$name" | cut -f7)/h)"
  echo "$(date -u +%FT%TZ) ecku $name $ecku" >>"$CFB_STATE/events.log"
}

cmd_pool() {
  local name="$1" cloud="$2" region="$3" cfu="$4" ttl="$5"
  [[ "$name" == loams-cfb-* ]] || die "names must start with loams-cfb-"
  local rate; rate="$(awk -v c="$cfu" 'BEGIN{printf "%.4f", c*0.21}')"
  budget_check "$(awk -v r="$rate" -v h="$ttl" 'BEGIN{printf "%.2f", r*h}')"
  local out; out="$(cc POST /fcpm/v2/compute-pools "$(jq -cn --arg n "$name" --arg c "${cloud^^}" --arg r "$region" --arg e "$CFB_ENV" --argjson m "$cfu" \
    '{spec:{display_name:$n, cloud:$c, region:$r, max_cfu:$m, environment:{id:$e}}}')")"
  [[ "$(cc_status)" == 20* ]] || die "create pool $name: HTTP $(cc_status) $(jq -c '.errors' <<<"$out")"
  local id t0 deadline; id="$(jq -r .id <<<"$out")"; t0="$(now)"; deadline=$((t0 + ttl * 3600))
  live_add flink-pool "$id" "$name" "$cloud/$region" "$t0" "$deadline" "$rate"
  cc_log_created flink-pool "$id" "$name" "$cloud/$region" "$deadline" "max_cfu $cfu; CFB benchmark"
  local dir="$CFB_STATE/pools/$name"; mkdir -p "$dir"; chmod 700 "$dir"
  jq '{id, name:.spec.display_name, cloud:.spec.cloud, region:.spec.region, max_cfu:.spec.max_cfu}' <<<"$out" >"$dir/pool.json"
  local fr; fr="$(cc GET "/fcpm/v2/regions?cloud=${cloud^^}&region_name=$region" | jq -r '.data[0].id')"
  local kid; kid="$(mint_key "$fr" "$name-flink" "$dir/key")"
  cc_log_created api-key "$kid" "$name-flink" "$cloud/$region" "$deadline" "owner Flink key for region $fr"
  live_add api-key "$kid" "$name-flink" "$cloud/$region" "$t0" "$deadline" 0
  echo "created pool $id $name ($cloud/$region max_cfu $cfu, \$$rate/h); Flink key $kid"
}

cmd_delete() {
  local name="$1" line
  line="$(live_get "$name")"; [[ -n "$line" ]] || die "$name is not live"
  IFS=$'\t' read -r kind id _ where created _ rate <<<"$line"
  local path
  case "$kind" in
    kafka-cluster) path="/cmk/v2/clusters/$id?environment=$CFB_ENV" ;;
    flink-pool) path="/fcpm/v2/compute-pools/$id?environment=$CFB_ENV" ;;
    api-key) path="/iam/v2/api-keys/$id" ;;
  esac
  cc DELETE "$path" >/dev/null
  [[ "$(cc_status)" == 2* || "$(cc_status)" == 404 ]] || die "delete $name: HTTP $(cc_status)"
  cc_log_deleted "$kind" "$id" "$name" "$where" "cfb.sh delete; HTTP $(cc_status)"
  printf '%s\t%s\t%s\n' "$created" "$(now)" "$rate" >>"$CFB_STATE/billed_done.tsv"
  grep -v -P "\t$id\t" "$live" >"$live.tmp"; mv "$live.tmp" "$live"
  # a pool's Flink key goes with it
  if [[ "$kind" == flink-pool ]] && [[ -n "$(live_get "$name-flink")" ]]; then cmd_delete "$name-flink"; fi
  echo "deleted $kind $id $name"
}

case "${1:-}" in
  cluster) shift; cmd_cluster "$@" ;;
  ecku) shift; cmd_ecku "$@" ;;
  pool) shift; cmd_pool "$@" ;;
  projected) shift; projected "${1:-0}"; echo ;;
  delete) shift; cmd_delete "$@" ;;
  list) column -t -s $'\t' "$live" ;;
  *) sed -n '2,15p' "$0"; exit 2 ;;
esac
