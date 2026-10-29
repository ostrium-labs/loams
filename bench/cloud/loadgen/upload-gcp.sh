# Uploads the raw results to gs://$BENCH_BUCKET/<run>/<row>/ (done.json last).
upload() {
  find "$1" -maxdepth 1 -type f ! -name done.json -print0 |
    xargs -0 -I{} gcloud storage cp --quiet {} "gs://$BENCH_BUCKET/$BENCH_RUN_ID/$BENCH_ROW/" &&
    gcloud storage cp --quiet "$1/done.json" "gs://$BENCH_BUCKET/$BENCH_RUN_ID/$BENCH_ROW/done.json"
}
