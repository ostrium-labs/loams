# Forces a Cloud Run cold start: a new revision (an env change) gets new instances.
# Cloud Run may start one at deploy time; the app's first/uptime_ms fields tell
# which probes really hit a cold instance. Needs BENCH_SERVICE and BENCH_REGION.
force_cold() {
  gcloud run services update "$BENCH_SERVICE" --region "$BENCH_REGION" \
    --update-env-vars "LOAMS_COLD=$1-$(date +%s)" --quiet >/dev/null
}
