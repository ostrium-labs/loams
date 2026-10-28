# The Java SDK leg (lane TSDK-java, D2130-D2134), sourced by run.sh when
# SDK_LEGS names `java`. io.temporal:temporal-sdk (pins.env
# TEMPORAL_SDK_JAVA, corpus/workers/java/pom.xml) built with Maven in the
# pinned MAVEN_IMAGE: the worker and the starter (`Generate generate`) run
# the scenarios against the gateway and export the histories themselves
# (the SDK's own protojson, which its replayer reads), and the SDK's
# WorkflowReplayer (CorpusReplayTest) checks them. Uses run.sh's leg, $rt,
# $addr, ${net[@]} (which carries TEMPORAL_API_KEY in profile mode),
# $corpus, $out and $SCENARIOS. Not yet run (honest status: CI's first run).
# shellcheck shell=bash

# java_run <name> <docker args...> -- <command in workers/java of a copy of
# the corpus> (the tests read ../../scenarios.json).
# The build step also resolves the test classpath; the Maven repository is a
# volume, so the second container reuses the first one's downloads.
java_run() {
  local name=$1
  shift
  local -a args=()
  while [ "$1" != -- ]; do args+=("$1"); shift; done
  shift
  "$rt" volume create dx1-m2cache >/dev/null 2>&1 || true
  leg "$name" "${args[@]}" -v "$corpus:/c:ro" -v dx1-m2cache:/root/.m2 "$MAVEN_IMAGE" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/workers/java && mvn -B -q -DskipTests package dependency:build-classpath -Dmdep.outputFile=target/cp.txt && $*"
}

leg_java_generate() {
  java_run java-generate "${net[@]}" -v "$out:/out" -- \
    "mvn -B -q test -Dtest=ScenariosTest -Dsurefire.failIfNoSpecifiedTests=false &&
     java -cp target/classes:\$(cat target/cp.txt) dev.loams.conformance.temporal.corpus.Generate generate \
       --address $addr --namespace default --no-search-attribute --scenarios $SCENARIOS --out /out/histories/java"
}

# The replayer never dials the server; it needs the network only for Maven.
leg_java_replay() {
  java_run java-replay "${net[@]}" -v "$out:/out:ro" -e CORPUS_HISTORIES=/out/histories/java -- \
    "mvn -B -q test -Dtest=CorpusReplayTest -Dsurefire.failIfNoSpecifiedTests=false"
}
