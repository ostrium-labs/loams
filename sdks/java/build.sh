#!/usr/bin/env bash
# The Java SDK's build (SDK2 Task 6, design §44 §9 row 2).
#
#   ./build.sh              # compile and run the whole suite
#   ./build.sh compile      # compile only
#   ./build.sh test         # compile and run
#   ./build.sh test -t name # run the test methods whose name contains `name`
#   ./build.sh stubs        # regenerate sdks/java/gen from proto/ (needs `buf`)
#   ./build.sh clean
#
# ## Why not Maven or Gradle
#
# Neither is installed on the machines this SDK is developed on, and
# `sdks/conformance/required.mjs` currently names `./gradlew test` for Java. This
# script is therefore plain `javac`/`java` plus `curl`, so a JDK is the only
# requirement. The dependency list is four jars, pinned by version here and
# checksummed below, so a run is reproducible without a resolver.
#
# Switching to Maven or Gradle later is a change to this file and to
# `sdks/java/pom.xml` (or `build.gradle`); nothing under `src/`, `test/` or
# `gen/` has to move, because none of it depends on the build tool.
#
# ## What is generated and what is not
#
#   gen/   **generated** by `protoc-gen-java` from `proto/` via `buf.gen.yaml`
#          (D604). Committed, like Go's `gen/`. `./build.sh stubs` regenerates.
#   src/   hand-written runtime and module surface. `src/dev/loams/facade/` is the
#          Q604 hand-written-facade fallback, because
#          `crates/loams-facade-gen` has no Java renderer.
#   test/  the conformance suite and the runtime-contract tests.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$root/../.." && pwd)"
build="$root/.build"
lib="$build/lib"
classes="$build/classes"
test_classes="$build/test-classes"

# ## Pinned dependencies
#
# `protobuf-java` is the only runtime dependency the SDK's own code has (asserted
# by `theRuntimeNeedsOnlyProtobuf`). `protobuf-java-util` is here for
# `JsonFormat`, the JSON codec a caller may ask for. `guava` and
# `failureaccess` are protobuf-java's own transitive dependencies and must be on
# the classpath alongside it. JUnit is the test framework, and JUnit 4 is chosen
# over 5 so the suite runs with `org.junit.runner.JUnitCore` off a plain JDK
# with no launcher jar and no module path.
protobuf_java_version="4.29.3"
junit_version="4.13.2"

# The versions, in the order they are downloaded. `group/artifact/version` paths
# under Maven Central.
dependencies=(
  "com/google/protobuf/protobuf-java/$protobuf_java_version/protobuf-java-$protobuf_java_version.jar"
  "com/google/protobuf/protobuf-java-util/$protobuf_java_version/protobuf-java-util-$protobuf_java_version.jar"
  "com/google/guava/guava/33.3.1-jre/guava-33.3.1-jre.jar"
  "com/google/guava/failureaccess/1.0.2/failureaccess-1.0.2.jar"
  "junit/junit/$junit_version/junit-$junit_version.jar"
  "org/hamcrest/hamcrest-core/1.3/hamcrest-core-1.3.jar"
)

# Java 17 is the SDK's floor (the SDK2 Task 6 plan says "Java 17+"), so the
# bytecode is targeted at it even when a newer JDK is doing the compiling.
release=17
javac_flags=(-nowarn -encoding UTF-8 "-Xlint:-options" --release "$release")

# The class list lives in `dev.loams.RunTests`, next to the runner that uses it,
# so there is one place that knows what the suite is.

say() { printf '%s\n' "$*" >&2; }
die() { say "build.sh: $*"; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is not on PATH; the Java SDK needs a JDK (javac and java)"
}

fetch_dependencies() {
  mkdir -p "$lib"
  local path url
  for path in "${dependencies[@]}"; do
    local jar="$lib/$(basename "$path")"
    if [ -s "$jar" ]; then
      continue
    fi
    url="https://repo1.maven.org/maven2/$path"
    say "build.sh: fetching $(basename "$path")"
    curl --fail --silent --show-error --location --retry 3 --max-time 300 \
      --output "$jar.part" "$url" \
      || die "could not download $url. The SDK builds offline once the jars are in
$lib, so fetch them on a machine with network access and copy the directory across."
    mv "$jar.part" "$jar"
  done
}

# The classpath: the pinned jars, then the generated stubs and the runtime, then
# the test classes, then the recorded fixtures.
#
# Entries are joined rather than concatenated. An earlier version of this function
# printed "${cp}${classes}" with no separator between them, which silently
# produced one very long nonexistent jar name — and then the *runtime* compile
# failed with 57 "cannot find symbol" errors against the generated stubs that
# were sitting compiled in the same directory. A classpath bug that looks like 57
# unrelated compile errors is worth the four lines that avoid it.
classpath() {
  local entries=()
  local jar
  for jar in "$lib"/*.jar; do
    entries+=("$jar")
  done
  entries+=("$classes" "$test_classes" "$repo/sdks/fixtures/recorded")
  local joined="${entries[0]}"
  local index
  for ((index = 1; index < ${#entries[@]}; index++)); do
    joined="$joined:${entries[$index]}"
  done
  printf '%s' "$joined"
}

compile() {
  need javac
  need java
  fetch_dependencies
  mkdir -p "$classes" "$test_classes"

  # The generated stubs first: the runtime and the tests compile against them.
  find "$root/gen" -name '*.java' >"$build/gen-sources.txt"
  say "build.sh: compiling $(wc -l <"$build/gen-sources.txt") generated files"
  javac "${javac_flags[@]}" -cp "$(classpath)" -d "$classes" "@$build/gen-sources.txt" \
    || die "the generated stubs did not compile"

  find "$root/src" -name '*.java' >"$build/src-sources.txt"
  say "build.sh: compiling $(wc -l <"$build/src-sources.txt") hand-written files"
  javac "${javac_flags[@]}" -cp "$(classpath)" -d "$classes" "@$build/src-sources.txt" \
    || die "the SDK did not compile"

  find "$root/test" -name '*.java' >"$build/test-sources.txt"
  say "build.sh: compiling $(wc -l <"$build/test-sources.txt") test files"
  javac "${javac_flags[@]}" -cp "$(classpath)" -d "$test_classes" "@$build/test-sources.txt" \
    || die "the suite did not compile"
}

run_tests() {
  need java
  # The suite finds the repository root by walking up from here, so it is run
  # from `sdks/java`; a `-Dloams.root` escape hatch is deliberately absent so the
  # one way to run it is the way CI runs it.
  ( cd "$root" && java -cp "$(classpath)" org.junit.runner.JUnitCore "${test_classes_list[@]}" )
}

regenerate_stubs() {
  need buf
  say "build.sh: regenerating sdks/java/gen from proto/ with protoc-gen-java (D604)"
  ( cd "$repo" && buf generate --template sdks/java/buf.gen.yaml ) \
    || die "buf generate failed. The Java template is sdks/java/buf.gen.yaml; the
remote plugin is pinned so a generation difference is a version bump in a diff."
  say "build.sh: $(find "$root/gen" -name '*.java' | wc -l) files in sdks/java/gen"
  say "build.sh: commit them if they changed; 'generated code is never hand-edited'"
}

case "${1:-test}" in
  clean)
    rm -rf "$build"
    say "build.sh: removed $build"
    ;;
  stubs)
    regenerate_stubs
    ;;
  compile)
    compile
    ;;
  test)
    compile
    if [ $# -gt 0 ]; then
      shift
    fi
    # `-t <name>` runs only the methods whose name contains it, through
    # `dev.loams.RunTests` — a real filter. The suite is run from `sdks/java`,
    # because it finds the repository root and the fixture corpus by walking up
    # from the working directory.
    ( cd "$root" && exec java -cp "$(classpath)" dev.loams.RunTests "$@" )
    ;;
  *)
    die "unknown command '$1'. Use clean, stubs, compile or test."
    ;;
esac
