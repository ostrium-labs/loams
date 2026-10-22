#!/bin/sh
# The engine's and `migrate`'s stores, chosen by the stack's profiles
# (COMPOSE_PROFILES in .env, passed in as LOAMS_PROFILES), then `exec loams`
# with the arguments compose gave and the store flags added:
#
# - without `tikv` (the default): the embedded metastore (openraft + redb,
#   one node, in /var/lib/loams/meta), Live on its embedded redb store and
#   Durable on SQLite, all in the loams-data volume;
# - with `tikv`: the metastore on TiKV (keyspace loams_meta) and Live on TiKV
#   (loams_live_dev). Durable stays on SQLite unless LOAMS_DURABLE_STORE names
#   another store (tikv://pd:2379/loams_durable: a fresh keyspace, nothing is
#   copied from SQLite).
#
# A flag already given (`migrate status --meta=…`) is not added again. The
# data directory records which store it was first started with
# (/var/lib/loams/.loams-store) and refuses the other one: switching would
# start on an empty catalog beside the data the other one describes.
set -eu

data=/var/lib/loams
marker="$data/.loams-store"

case ",${LOAMS_PROFILES:-}," in
  *,tikv,*) store=tikv ;;
  *) store=embedded ;;
esac

if [ -f "$marker" ]; then
  was="$(cat "$marker")"
  if [ "$was" != "$store" ]; then
    echo "loams: this stack's data was started on the $was store, and COMPOSE_PROFILES now selects $store." >&2
    if [ "$was" = tikv ]; then
      echo "loams: add tikv to COMPOSE_PROFILES in .env (the catalog is in TiKV)." >&2
    else
      echo "loams: remove tikv from COMPOSE_PROFILES in .env (the catalog is in the loams-data volume)." >&2
    fi
    exit 78
  fi
fi

# Whether flag $1 is among the remaining arguments.
given() {
  flag="$1"
  shift
  for arg in "$@"; do
    case "$arg" in
      "$flag" | "$flag"=*) return 0 ;;
    esac
  done
  return 1
}

role="${1:-}"
if [ "$store" = tikv ]; then
  given --meta "$@" || set -- "$@" --meta=tikv://pd:2379/loams_meta
  given --live-store "$@" || set -- "$@" --live-store=tikv://pd:2379/loams_live_dev
fi
if [ "$role" = standalone ] && [ -n "${LOAMS_DURABLE_STORE:-}" ]; then
  given --durable-store "$@" || set -- "$@" "--durable-store=$LOAMS_DURABLE_STORE"
fi

# The engine and `migrate run` claim the data directory for this store.
if [ ! -f "$marker" ] && { [ "$role" = standalone ] || [ "${2:-}" = run ]; }; then
  printf '%s\n' "$store" > "$marker"
fi

# A running engine holds Durable's SQLite store (durable/durable.lock) and,
# on the embedded store, the metastore (meta/meta.redb), and it passed the
# startup gate on these migrations when it started. So `migrate run` while
# it runs (a second `docker compose up`, or compose starting a dependant
# such as the House) has nothing to do and could not open them: skip, and
# let the dependants start. An engine of a newer release that compose then
# recreates still refuses data with a pending migration (the startup gate);
# `loams upgrade` stops the engine before it migrates.
held() { [ -f "$1" ] && ! flock -n "$1" true; }
if [ "$role" = migrate ] && [ "${2:-}" = run ]; then
  if held "$data/durable/durable.lock" \
    || { [ "$store" = embedded ] && held "$data/meta/meta.redb"; }; then
    echo "loams migrate: the running engine holds its stores; nothing to migrate now (stop the engine to migrate)" >&2
    exit 0
  fi
fi

exec /usr/local/bin/loams "$@"
