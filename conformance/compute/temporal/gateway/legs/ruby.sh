# The Ruby SDK leg (lane TSDK-ruby, D2150-D2154), sourced by run.sh when
# SDK_LEGS names `ruby` (TSDK-python's hook, D2152). The temporalio gem
# (pins.env TEMPORAL_SDK_RUBY, on sdk-core; locked in
# corpus/workers/ruby/Gemfile.lock) under the pinned bundler (BUNDLER_VERSION)
# in the pinned RUBY_IMAGE: the worker and the starter (generate.rb) run the
# scenarios against the gateway, the Go exporter writes the histories (as for
# TypeScript), and the gem's WorkflowReplayer checks them (replay.rb). Uses
# run.sh's leg, go_run, $rt, $addr, ${net[@]}, $corpus, $out, $SCENARIOS.
# Not yet run (honest status: CI's first run).
# shellcheck shell=bash

# The image's GEM_HOME (/usr/local/bundle) is the volume dx1-gemcache, so the
# pinned bundler and the gems the generator installs are there for the
# replay, which runs offline (--network none).
# rb_run <name> <docker args...> -- <command in workers/ruby>
rb_run() {
  local name=$1
  shift
  local -a args=()
  while [ "$1" != -- ]; do args+=("$1"); shift; done
  shift
  leg "$name" "${args[@]}" -v "$corpus:/c:ro" -v dx1-gemcache:/usr/local/bundle \
    -e BUNDLE_FROZEN=true -e "BUNDLER_VERSION=$BUNDLER_VERSION" "$RUBY_IMAGE" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/workers/ruby &&
      (gem list -i bundler -v $BUNDLER_VERSION >/dev/null || gem install --no-document bundler -v $BUNDLER_VERSION >/dev/null) &&
      (bundle install --quiet --local 2>/dev/null || bundle install --quiet) && $*"
}

leg_ruby_generate() {
  "$rt" volume create dx1-gemcache >/dev/null 2>&1 || true
  rb_run rb-generate "${net[@]}" -v "$out:/out" -- \
    "bundle exec ruby test/scenarios_test.rb &&
     bundle exec ruby generate.rb all $addr default /out/generate-ruby.json $SCENARIOS" &&
    go_run ruby-export "workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $addr -namespace default -no-search-attribute -prefix corpus-ruby- -scenarios $SCENARIOS -out /out/histories/ruby"
}

# The replayer never dials the server; the gems are in dx1-gemcache already.
leg_ruby_replay() {
  rb_run rb-replay --network none -v "$out:/out:ro" -e CORPUS_HISTORIES=/out/histories/ruby -- \
    "bundle exec ruby replay.rb"
}
