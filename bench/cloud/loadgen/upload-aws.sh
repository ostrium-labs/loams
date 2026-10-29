# Uploads the raw results to s3://$BENCH_BUCKET/<run>/<row>/ (done.json last).
upload() {
  aws s3 cp --recursive --exclude done.json --only-show-errors "$1" "s3://$BENCH_BUCKET/$BENCH_RUN_ID/$BENCH_ROW/" &&
    aws s3 cp --only-show-errors "$1/done.json" "s3://$BENCH_BUCKET/$BENCH_RUN_ID/$BENCH_ROW/done.json"
}
