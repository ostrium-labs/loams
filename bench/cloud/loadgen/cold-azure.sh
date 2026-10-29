# Forces an Azure Functions cold start: an app-setting change recycles the app's
# instances. Uses the VM's managed identity (Website Contributor on the app only)
# through Azure Resource Manager; the Ubuntu image has no az CLI.
# Needs BENCH_FUNCTION_APP_ID (the app's resource id).
azure_token() {
  curl -sf -H Metadata:true "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=$1" | jq -r .access_token
}
force_once() {
  local tok api="api-version=2024-04-01" settings
  tok=$(azure_token https://management.azure.com/) || return 1
  settings=$(curl -sf -X POST -H "Authorization: Bearer $tok" -H 'content-length: 0' \
    "https://management.azure.com${BENCH_FUNCTION_APP_ID}/config/appsettings/list?$api") || return 1
  curl -sf -X PUT -H "Authorization: Bearer $tok" -H 'content-type: application/json' \
    --data "$(jq -c --arg n "$1-$(date +%s)" '{properties: (.properties + {LOAMS_COLD: $n})}' <<<"$settings")" \
    "https://management.azure.com${BENCH_FUNCTION_APP_ID}/config/appsettings?$api" >/dev/null || return 1
  sleep "${BENCH_AZURE_COLD_WAIT:-30}"
}
# The VM's role assignment can take minutes to propagate after boot: retry a
# refused change for up to five minutes before counting the probe as not forced.
force_cold() {
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    force_once "$1" && return 0
    sleep 30
  done
  return 1
}
