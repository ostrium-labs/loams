#!/usr/bin/env bash
# A stand-in for podman/docker in LocalRuntime's unit tests (tests/it/engine.rs).
# Usage: fake-engine.sh <state dir> <engine args...>. Containers are
# directories under <state dir>/c/<name> with `labels` and `status` files.
# Switches (files in <state dir>): fail-run (run leaves the container
# `created` and fails), stop-delay (seconds `stop` takes), vanish-on-inspect
# (a multi-container inspect loses its first container and fails once),
# job-exit (exit code of `run --rm`).
set -u
D="$1"; shift
mkdir -p "$D/c"
echo "$*" >> "$D/calls.log"
cmd="$1"; shift
case "$cmd" in
  --version) echo "fake-engine 1" ;;
  run)
    name=""; labels=(); rm=0
    while [ $# -gt 0 ]; do
      case "$1" in
        -d) ;;
        --rm) rm=1 ;;
        --name) name="$2"; shift ;;
        --label) labels+=("$2"); shift ;;
        --network|--memory|--memory-swap|--cpus|-v|-e) shift ;;
        *) break ;;
      esac
      shift
    done
    if [ -e "$D/c/$name" ]; then echo "Error: the container name \"$name\" is already in use" >&2; exit 125; fi
    mkdir -p "$D/c/$name"
    printf '%s\n' "${labels[@]}" > "$D/c/$name/labels"
    if [ -e "$D/fail-run" ]; then
      echo created > "$D/c/$name/status"
      echo "Error: fake start failure" >&2
      exit 126
    fi
    echo running > "$D/c/$name/status"
    if [ "$rm" = 1 ]; then
      rm -rf "${D:?}/c/$name"
      exit "$(cat "$D/job-exit" 2>/dev/null || echo 0)"
    fi
    echo "$name"
    ;;
  ps)
    filters=()
    while [ $# -gt 0 ]; do
      case "$1" in --filter) filters+=("${2#label=}"); shift ;; esac
      shift
    done
    for c in "$D"/c/*/; do
      [ -d "$c" ] || continue
      ok=1
      for f in "${filters[@]}"; do grep -qxF "$f" "$c/labels" || ok=0; done
      if [ "$ok" = 1 ]; then basename "$c"; fi
    done
    ;;
  inspect)
    shift 2
    if [ $# -gt 1 ] && [ -e "$D/vanish-on-inspect" ]; then
      rm -f "$D/vanish-on-inspect"
      rm -rf "${D:?}/c/$1"
      echo "Error: no such container $1" >&2
      exit 125
    fi
    for n in "$@"; do [ -d "$D/c/$n" ] || { echo "Error: no such container $n" >&2; exit 125; }; done
    for n in "$@"; do
      m=$(sed -n 's/^io.loams.sqldb.member=//p' "$D/c/$n/labels")
      f=$(sed -n 's/^io.loams.sqldb.fingerprint=//p' "$D/c/$n/labels")
      printf '%s\t%s\t%s\t%s\t%s\n' "$n" "$(cat "$D/c/$n/status")" 0 "$m" "$f"
    done
    ;;
  stop)
    shift 2
    sleep "$(cat "$D/stop-delay" 2>/dev/null || echo 0)"
    for n in "$@"; do [ -d "$D/c/$n" ] && echo exited > "$D/c/$n/status"; done
    true
    ;;
  rm)
    [ "${1:-}" = -f ] && shift
    for n in "$@"; do rm -rf "${D:?}/c/$n"; done
    ;;
  logs) ;;
  *) echo "fake-engine: unknown command $cmd" >&2; exit 2 ;;
esac
