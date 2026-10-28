#!/usr/bin/env bash
# Generate the Java half of the history corpus (plan DX1, lane TSDK-java,
# D2131) against a running upstream Temporal server on the stack's network,
# into histories/java/, and rebuild manifest.json's Java entries.
#
#   generate-java.sh [--out DIR]   # default: this directory's histories/ (the committed corpus)
#
# Environment: as generate.sh. TEMPORAL_ADDRESS (temporal-frontend:7233),
# CORPUS_SERVER (compat-mode: the stock server on Loams Postgres, the
# default; start-dev: the CLI's dev server), COMPUTE_PG_MAJOR (16),
# OC_NETWORK (the network the server is on; `host` for a loopback server).
# The Java worker (workers/java, Maven, io.temporal:temporal-sdk at
# TEMPORAL_SDK_JAVA) runs in the pinned MAVEN_IMAGE; it registers a fresh
# namespace and the corpus's search attribute, drives every scenario with
# the fixed workflow ID corpus-java-<scenario>, and exports the histories
# itself (the SDK's own protojson, which its replayer reads).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

out=$here/histories
[ "${1:-}" = --out ] && out=${2:?--out DIR}
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}" "${CORPUS_SERVER:=compat-mode}" "${COMPUTE_PG_MAJOR:=16}"
rt=$(oc_container_cmd)
ns=corpus-java-$(date +%s)
mkdir -p "$out/java"
rm -f "$out"/java/*.json "$out"/java/*.binpb "$out/generate-java.json"
"$rt" volume create dx1-m2cache >/dev/null 2>&1 || true

# java_run <command in workers/java>: the pinned Maven image on a copy of
# the corpus directory (mounted read-only).
java_run() {
  "$rt" run --rm --network "$OC_NETWORK" -v "$here:/c:ro" -v "$out:/out" -v dx1-m2cache:/root/.m2 \
    -e TEMPORAL_API_KEY "$MAVEN_IMAGE" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/workers/java && mvn -B -q -DskipTests package dependency:build-classpath -Dmdep.outputFile=target/cp.txt && $1"
}

java_run "java -cp target/classes:\$(cat target/cp.txt) dev.loams.conformance.temporal.corpus.Generate generate --address $TEMPORAL_ADDRESS --namespace $ns --out /out/java"

if [ "$out" = "$here/histories" ]; then
  case $CORPUS_SERVER in
    compat-mode) version=$TEMPORAL_VERSION image=$TEMPORAL_SERVER_IMAGE pg=$COMPUTE_PG_MAJOR ;;
    # CLI 1.9.1 embeds server 1.32.0 (`temporal --version`), not the pinned 1.32.1.
    start-dev) version=1.32.0 image="temporal server start-dev (CLI $TEMPORAL_CLI in $TEMPORAL_ADMIN_TOOLS_IMAGE)" pg=start-dev ;;
    *) echo "generate-java.sh: CORPUS_SERVER is compat-mode or start-dev" >&2; exit 2 ;;
  esac
  python3 "$here/manifest.py" build --server-version "$version" --server-image "$image" --pg-major "$pg" --languages java
fi
echo "generate-java.sh: namespace $ns, $(find "$out/java" -name "*.json" | wc -l) Java histories in $out/java"
