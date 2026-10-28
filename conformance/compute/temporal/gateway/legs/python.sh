# The Python SDK leg (lane TSDK-python, D2135-D2138), sourced by run.sh when
# SDK_LEGS names `python`. temporalio (pins.env TEMPORAL_SDK_PY, locked in
# corpus/workers/python/uv.lock) under uv (UV_VERSION) in the pinned
# PYTHON_IMAGE: the worker and the starter run the scenarios against the
# gateway, the Go exporter writes the histories (as for TypeScript), and the
# SDK's Replayer checks them. Uses run.sh's leg, go_run, $addr, ${net[@]},
# $corpus, $out, $SCENARIOS. Not yet run (honest status: CI's first run).
# shellcheck shell=bash

# py_run <name> <docker args...> -- <command in workers/python>
py_run() {
  local name=$1
  shift
  local -a args=()
  while [ "$1" != -- ]; do args+=("$1"); shift; done
  shift
  leg "$name" "${args[@]}" -v "$corpus:/c:ro" -v dx1-uvcache:/root/.cache \
    -e UV_PYTHON_DOWNLOADS=never -e UV_LINK_MODE=copy -e PYTHONUNBUFFERED=1 "$PYTHON_IMAGE" \
    sh -ec "pip install --quiet --disable-pip-version-check --root-user-action=ignore uv==$UV_VERSION &&
      cp -r /c /tmp/c && cd /tmp/c/workers/python && uv sync --frozen --quiet && $*"
}

leg_python_generate() {
  "$rt" volume create dx1-uvcache >/dev/null 2>&1 || true
  py_run py-generate "${net[@]}" -v "$out:/out" -- \
    "uv run --frozen python -m unittest discover -s tests &&
     uv run --frozen python -m corpus_py generate $addr default /out/generate-python.json $SCENARIOS" &&
    go_run python-export "workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $addr -namespace default -no-search-attribute -prefix corpus-python- -scenarios $SCENARIOS -out /out/histories/python"
}

# The replayer never dials the server; it needs the network only for pip.
leg_python_replay() {
  py_run py-replay "${net[@]}" -v "$out:/out:ro" -e CORPUS_HISTORIES=/out/histories/python -- \
    "uv run --frozen python -m corpus_py replay"
}
