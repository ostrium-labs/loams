# Forces a Lambda cold start: a configuration change retires every warm
# environment. Needs BENCH_FUNCTION; the loadgen's role may change only it.
force_cold() {
  local vars
  vars=$(aws lambda get-function-configuration --function-name "$BENCH_FUNCTION" --query 'Environment.Variables' --output json) || return 1
  aws lambda update-function-configuration --function-name "$BENCH_FUNCTION" \
    --environment "$(jq -c --arg n "$1-$(date +%s)" '{Variables: (. + {LOAMS_COLD: $n})}' <<<"$vars")" >/dev/null || return 1
  aws lambda wait function-updated-v2 --function-name "$BENCH_FUNCTION"
}
