# The PHP SDK leg (lane TSDK-php, D2145-D2149), sourced by run.sh when
# SDK_LEGS names `php` (TSDK-python's hook). temporal/sdk (pins.env
# TEMPORAL_SDK_PHP, locked with every dependency in
# corpus/workers/php/composer.lock) under RoadRunner (ROADRUNNER_VERSION)
# with its temporal plugin, in the corpus's PHP image
# (corpus/workers/php/Containerfile, built locally once): RoadRunner runs the
# worker, the starter (starter.php) drives the scenarios against the gateway,
# the Go exporter writes the histories (as for TypeScript), and the SDK's
# WorkflowReplayer (replay.php, through RoadRunner's replay RPC) checks
# them. Uses run.sh's leg, go_run, $rt, $addr, ${net[@]}, $corpus, $out,
# $SCENARIOS. Not yet run (honest status: CI's first run).
# shellcheck shell=bash
# shellcheck source=conformance/compute/temporal/corpus/workers/php/image.sh
. "$corpus/workers/php/image.sh"

# php_run <name> <docker args...> -- <command in workers/php>: composer
# install from the lock (cache in the volume dx1-composercache), then the
# command. RoadRunner's RPC port is off 6001, which loopback mode's host
# network may already use.
php_run() {
  local name=$1 image
  shift
  local -a args=()
  while [ "$1" != -- ]; do args+=("$1"); shift; done
  shift
  image=$(php_corpus_image "$rt")
  leg "$name" "${args[@]}" -v "$corpus:/c:ro" -v dx1-composercache:/composer/cache \
    -e RR_RPC_PORT=36001 "$image" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/workers/php &&
      composer install --no-interaction --no-progress --prefer-dist --quiet && $*"
}

leg_php_generate() {
  "$rt" volume create dx1-composercache >/dev/null 2>&1 || true
  php_run php-generate "${net[@]}" -v "$out:/out" -- \
    "./corpus-php.sh check && ./corpus-php.sh generate $addr default /out/generate-php.json $SCENARIOS" &&
    go_run php-export "workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $addr -namespace default -no-search-attribute -prefix corpus-php- -scenarios $SCENARIOS -out /out/histories/php"
}

# RoadRunner's client dials the frontend at start, so the replay runs on the
# leg's network too; the replay itself sends the server nothing.
leg_php_replay() {
  php_run php-replay "${net[@]}" -v "$out:/out:ro" -e CORPUS_HISTORIES=/out/histories/php -- \
    "./corpus-php.sh replay $addr default"
}
