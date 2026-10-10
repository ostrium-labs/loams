#!/usr/bin/env bash
# The Kotlin SDK's build (SDK2 Task 5, design §44 §9 row 8).
#
#   ./build.sh                 # compile and run the whole suite
#   ./build.sh compile         # compile only
#   ./build.sh test            # compile and run
#   ./build.sh test -t name    # run only the tests whose name contains `name`
#   ./build.sh descriptors     # regenerate gen/loams-descriptor.binpb from proto/ (needs `buf`)
#   ./build.sh clean
#
# ## Why not Gradle
#
# `sdks/conformance/required.mjs` names `./gradlew test` for Kotlin. **There is no
# Gradle on the machines this SDK is developed on and no wrapper that can be made
# to run** (a wrapper bootstraps by downloading a distribution, which this
# environment's egress does not permit for `services.gradle.org`), so this script
# is `kotlinc` plus `java` plus `curl`. That is the same ruling `sdks/java`
# reached for `mvn`/`gradle` (its `build.sh`, and this file's sibling
# `DEPENDENCIES.md`), and the reason `runOne` in `required.mjs` has to name
# `./build.sh` rather than `./gradlew`.
#
# Switching to Gradle later is a change to this file and to `build.gradle.kts`;
# nothing under `src/` has to move, because none of it depends on the build tool.
#
# ## Low RAM
#
# `kotlinc` is a compiler that runs in-process and will happily take a default
# heap larger than the machine has. `-J-Xmx1g` is therefore passed explicitly on
# every invocation rather than left to the default, because the machine this SDK
# is developed on has 15 GiB and has kernel-OOM-killed an unrelated desktop
# session. The compiler is never run concurrently with anything else here.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$root/../.." && pwd)"
build="$root/.build"
lib="$build/lib"
classes="$build/classes"
test_classes="$build/test-classes"

## Pinned dependencies.
#
# `protobuf-java` is the SDK's **only** runtime dependency (asserted by
# `RuntimeContractTest.theRuntimeNeedsOnlyProtobuf`): the message codec, the
# descriptor walk `Idempotency.apply` uses to read `idempotency_key` off a
# *schema* rather than off an object (R3), and the `FileDescriptor` graph the
# facade is **derived** from (D652). It is BSD-3-Clause and its 4.x line has no
# transitive dependency of its own, so guava is not on this classpath — verified,
# not assumed: `build.sh` compiles and runs the suite with this jar alone.
#
# The test framework is a 60-line harness in `src/test/kotlin` rather than JUnit,
# for the same reason: JUnit would need a launcher on a plain JDK and `run-test.sh`
# already had to be taught to distrust a runner that reports success without
# having run anything (D651).
protobuf_java_version="4.35.1"
protobuf_java_jar="protobuf-java-${protobuf_java_version}.jar"
# Verified 2026-10-06 against the artifact Maven Central served for this path.
# A mismatch is a build failure, not a warning: a jar that is not the one this file
# names is a jar nobody audited.
protobuf_java_sha256="a4345ba2aa009912ff6f90467fea2d104605256b72c50840d75f13256638a472"
protobuf_java_path="com/google/protobuf/protobuf-java/${protobuf_java_version}/${protobuf_java_jar}"

kotlin_heap="-J-Xmx1g"

say() { printf '%s\n' "$*" >&2; }
die() { say "build.sh: $*"; exit 1; }

# `kotlinc` is not on PATH in the environment this SDK is developed in. Look for
# it on PATH first, then in the toolchain directory the project's
# `install-toolchains.sh` puts it in, and say which one was used — a build that
# silently used a different compiler than the one it names is a build whose
# results nobody can reproduce.
find_kotlinc() {
  if command -v kotlinc >/dev/null 2>&1; then
    command -v kotlinc
    return 0
  fi
  local candidate="$HOME/.local/toolchains/kotlinc/kotlinc/bin/kotlinc"
  if [ -x "$candidate" ]; then
    printf '%s' "$candidate"
    return 0
  fi
  return 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is not on PATH; the Kotlin SDK needs a JDK (javac and java) and kotlinc"
}

fetch_dependencies() {
  mkdir -p "$lib"
  local jar="$lib/$protobuf_java_jar"
  if [ -s "$jar" ]; then
    return 0
  fi
  local url="https://repo1.maven.org/maven2/$protobuf_java_path"
  say "build.sh: fetching $protobuf_java_jar"
  if ! curl --fail --silent --show-error --location --retry 3 --max-time 300 \
      --output "$jar.part" "$url"; then
    rm -f "$jar.part"
    die "could not download $url. The SDK builds offline once the jar is in
$lib, so fetch it on a machine with network access and copy the directory across."
  fi
  if [ "$protobuf_java_sha256" != "PLACEHOLDER_SHA256" ]; then
    local got
    got="$(sha256sum "$jar.part" | cut -d' ' -f1)"
    [ "$got" = "$protobuf_java_sha256" ] || {
      rm -f "$jar.part"
      die "$protobuf_java_jar has sha256 $got, and the pin in this file says $protobuf_java_sha256"
    }
  fi
  mv "$jar.part" "$jar"
}

# The classpath: the pinned jar, then the committed descriptor set's classes,
# then the SDK, then the test classes.
#
# Entries are **joined** with `:`. An earlier version of the sibling Java SDK's
# classpath function concatenated instead, which silently produced one very long
# nonexistent jar name; a classpath bug that looks like dozens of unrelated
# "cannot find symbol" errors is worth the four lines that avoid it.
classpath() {
  local entries=()
  local jar
  for jar in "$lib"/*.jar; do
    [ -f "$jar" ] && entries+=("$jar")
  done
  local kotlinc_path
  kotlinc_path="$(find_kotlinc 2>/dev/null || true)"
  if [ -n "$kotlinc_path" ]; then
    local klib="$(dirname "$kotlinc_path")/../lib"
    for jar in "$klib"/kotlin-stdlib*.jar; do
      [ -f "$jar" ] && entries+=("$jar")
    done
  fi
  entries+=("$classes" "$test_classes")
  local joined="${entries[0]}"
  local index
  for ((index = 1; index < ${#entries[@]}; index++)); do
    joined="$joined:${entries[$index]}"
  done
  printf '%s' "$joined"
}

compile() {
  need java
  need javac
  fetch_dependencies
  mkdir -p "$classes" "$test_classes"

  local kotlinc_path
  kotlinc_path="$(find_kotlinc)" || die "kotlinc is not on PATH and not at \$HOME/.local/toolchains/kotlinc/kotlinc/bin/kotlinc.
Run scripts/install-toolchains.sh, or put kotlinc on PATH."
  say "build.sh: $(command -v kotlinc >/dev/null 2>&1 && kotlinc -version 2>&1 | head -1 || "$kotlinc_path" -version 2>&1 | head -1)"

  # `find` rather than a shell glob, so the source list is a file the compiler
  # reads and a path with a space in it cannot split into two arguments.
  # The committed descriptor set goes onto the classpath as a resource, which is
  # how `dev.loams.Descriptors` finds it: one `FileDescriptorSet` read at
  # class-initialisation, and every binding, message type and module name below it
  # is derived from that graph (D652) rather than transcribed from the protos.
  [ -s "$root/gen/loams-descriptor.binpb" ] \
    || die "gen/loams-descriptor.binpb is missing or empty. It is the committed
FileDescriptorSet the SDK reads; run './build.sh descriptors' to regenerate it."
  cp "$root/gen/loams-descriptor.binpb" "$classes/loams-descriptor.binpb"

  if [ ! -d "$root/src/main/kotlin" ]; then
    die "there is no src/main/kotlin. The suite is written against dev.loams, and
nothing implements it yet — this is the state the tests were committed in."
  fi

  find "$root/src/main/kotlin" -name '*.kt' | sort >"$build/main-sources.txt"
  say "build.sh: compiling $(wc -l <"$build/main-sources.txt") SDK files"
  "$kotlinc_path" $kotlin_heap -nowarn -jvm-target 11 \
    -cp "$lib/$protobuf_java_jar" -d "$classes" "@$build/main-sources.txt" \
    || die "the SDK did not compile"

  find "$root/src/test/kotlin" -name '*.kt' | sort >"$build/test-sources.txt"
  say "build.sh: compiling $(wc -l <"$build/test-sources.txt") test files"
  "$kotlinc_path" $kotlin_heap -nowarn -jvm-target 11 \
    -cp "$lib/$protobuf_java_jar:$classes" -d "$test_classes" "@$build/test-sources.txt" \
    || die "the suite did not compile"
}

run_tests() {
  need java
  # The suite finds the repository root by walking up from the working
  # directory, so it is run from `sdks/kotlin` — the one way to run it, which is
  # the way CI runs it.
  ( cd "$root" && exec java -cp "$(classpath)" dev.loams.test.RunTests "$@" )
}

regenerate_descriptors() {
  need buf
  say "build.sh: regenerating gen/loams-descriptor.binpb from proto/ (D652)"
  ( cd "$repo" && buf build -o "$root/gen/loams-descriptor.binpb" ) \
    || die "buf build failed. The input is proto/; the output is a FileDescriptorSet
that the SDK reads at run time, which is what lets the facade be derived rather
than transcribed (D652)."
  say "build.sh: $(wc -c <"$root/gen/loams-descriptor.binpb") bytes in gen/loams-descriptor.binpb"
  say "build.sh: commit it if it changed; the descriptor set is generated, never hand-edited"
}

case "${1:-test}" in
  clean)
    rm -rf "$build"
    say "build.sh: removed $build"
    ;;
  descriptors)
    regenerate_descriptors
    ;;
  compile)
    compile
    ;;
  test)
    compile
    shift || true
    run_tests "$@"
    ;;
  *)
    die "unknown command '$1'. Use clean, descriptors, compile or test."
    ;;
esac