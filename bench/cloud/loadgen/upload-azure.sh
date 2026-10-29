# Uploads the raw results to the results container with the VM's managed identity
# (Storage Blob Data Contributor on that container), done.json last.
upload() {
  local tok f ok=0
  tok=$(curl -sf -H Metadata:true "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=https://storage.azure.com/" | jq -r .access_token) || return 1
  put() {
    curl -sf -X PUT -H "Authorization: Bearer $tok" -H 'x-ms-version: 2023-11-03' -H 'x-ms-blob-type: BlockBlob' \
      --data-binary @"$1" "${BENCH_BLOB_CONTAINER_URL%/}/$BENCH_RUN_ID/$BENCH_ROW/$(basename "$1")" >/dev/null
  }
  for f in "$1"/*; do [[ $(basename "$f") == done.json ]] || put "$f" || ok=1; done
  ((ok == 0)) && put "$1/done.json"
}
