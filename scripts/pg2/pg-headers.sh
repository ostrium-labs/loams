#!/usr/bin/env bash
# Extract the Postgres server headers that the Neon fork's postgres_ffi
# builds its bindings from (PG2 Task 31, ruling R31.6), from a pinned image,
# into <out>/vNN/include/postgresql/server. Point POSTGRES_INSTALL_DIR at
# <out>.
#
#   scripts/pg2/pg-headers.sh <image@sha256:digest> pg_install
#   scripts/pg2/pg-headers.sh --self-test
#
# The storage image (`loams-postgres-storage`, or Neon's `neon` image) holds
# every major it was built with under /usr/local/vNN/include/postgresql/server;
# the compute images hold none, so the storage image is the source. The image
# must be pinned by digest. CONTAINER picks the engine (docker by default;
# podman works).
#
# Majors: v14 to v19 are extracted when the image has them (NF2 Task 4; 19 is
# listed for NF2b). The required majors are those the pin file offers
# (deploy/pins/loams-postgres-images.toml: supported, beta and maintenance),
# so 18 is required as soon as the pin file lists it, and from then on an
# image without 18's headers fails here instead of in bindgen. PG_HEADERS_REQUIRE
# ("17 18") overrides the list, e.g. to check out a fork branch that binds 18
# before the pin file offers it. 17 is always required.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
all_majors="14 15 16 17 18 19"

# The majors the pin file offers, space-separated, sorted.
pinned_majors() {
  local pins=${PG_HEADERS_PINS:-$here/../../deploy/pins/loams-postgres-images.toml}
  [ -f "$pins" ] || return 0
  python3 - "$pins" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as f:
    majors = tomllib.load(f).get("majors", {})
out = set()
for key in ("supported", "beta", "maintenance"):
    out.update(int(m) for m in majors.get(key, []))
print(" ".join(str(m) for m in sorted(out)))
PY
}

required_majors() {
  local req=${PG_HEADERS_REQUIRE:-$(pinned_majors)}
  # 17 is always required; dedupe and sort.
  printf '%s\n' 17 $req | sort -un | tr '\n' ' ' | sed 's/ $//'
}

extract() {
  local image=$1 out=$2 engine=${CONTAINER:-docker}
  case "$image" in
    *@sha256:[0-9a-f]*) ;;
    *) echo "pg-headers: $image is not pinned by digest" >&2; return 2 ;;
  esac

  local id
  id=$("$engine" create "$image" /bin/true)
  # shellcheck disable=SC2064 # expand now: the trap runs after locals are gone
  trap "\"$engine\" rm -f \"$id\" >/dev/null" EXIT

  local found=0 v src
  for v in $all_majors; do
    src=/usr/local/v$v/include/postgresql/server
    if "$engine" cp "$id:$src" - >/dev/null 2>&1; then
      mkdir -p "$out/v$v/include/postgresql"
      rm -rf "$out/v$v/include/postgresql/server"
      "$engine" cp "$id:$src" "$out/v$v/include/postgresql/server"
      echo "pg-headers: v$v -> $out/v$v/include/postgresql/server"
      found=$((found + 1))
    fi
  done

  local missing=""
  for v in $(required_majors); do
    [ -f "$out/v$v/include/postgresql/server/postgres.h" ] || missing="$missing $v"
  done
  if [ -n "$missing" ]; then
    echo "pg-headers: no Postgres server headers for major(s)$missing in $image (required: $(required_majors))" >&2
    return 1
  fi
  echo "pg-headers: $found major version(s); required $(required_majors) present"
}

self_test() {
  local t fail=0
  t=$(mktemp -d "${TMPDIR:-/tmp}/pg-headers-test.XXXXXX")
  # shellcheck disable=SC2064
  trap "rm -rf '$t'" EXIT

  # A fake engine: the "image" is a directory tree under $t/img.
  mkdir -p "$t/bin"
  cat >"$t/bin/engine" <<'SH'
#!/usr/bin/env bash
root=$FAKE_IMAGE_ROOT
case "$1" in
  create) echo fakeid ;;
  rm) ;;
  cp)
    src=${2#fakeid:}
    [ -d "$root$src" ] || exit 1
    if [ "$3" = - ]; then exit 0; fi
    cp -r "$root$src" "$3" ;;
  *) exit 3 ;;
esac
SH
  chmod +x "$t/bin/engine"

  image_with() { # majors...
    rm -rf "$t/img"
    local v
    for v in "$@"; do
      mkdir -p "$t/img/usr/local/v$v/include/postgresql/server"
      echo "/* $v */" >"$t/img/usr/local/v$v/include/postgresql/server/postgres.h"
    done
  }
  pins_with() { # supported beta maintenance
    printf 'schema = 1\n[majors]\nsupported = [%s]\nbeta = [%s]\nmaintenance = [%s]\n' "$1" "$2" "$3" >"$t/pins.toml"
  }
  run() { # expected-status name
    local want=$1 name=$2 got=0
    rm -rf "$t/out"
    (
      export CONTAINER="$t/bin/engine" FAKE_IMAGE_ROOT="$t/img" PG_HEADERS_PINS="$t/pins.toml"
      extract "img@sha256:0123abcd" "$t/out"
    ) >"$t/log" 2>&1 || got=$?
    if [ "$got" -ne "$want" ]; then
      echo "FAIL $name: status $got, want $want"; sed 's/^/  /' "$t/log"; fail=1
    else
      echo "ok   $name"
    fi
  }

  pins_with "17" "" "16"
  image_with 14 15 16 17
  run 0 "pin file 16+17, image 14-17"
  image_with 17
  run 1 "pin file requires 16, image lacks it"
  pins_with "17" "18" "16"
  image_with 14 15 16 17
  run 1 "18 in beta, image without 18 fails"
  grep -q "major(s) 18 " "$t/log" || { echo "FAIL the error names 18"; fail=1; }
  image_with 16 17 18
  run 0 "18 in beta, image with 18"
  image_with 16 17 18 19
  run 0 "19 is extracted when present"
  [ -f "$t/out/v19/include/postgresql/server/postgres.h" ] || { echo "FAIL v19 extracted"; fail=1; }
  pins_with "17" "" ""
  image_with 17
  PG_HEADERS_REQUIRE="17 18" run 1 "PG_HEADERS_REQUIRE adds 18"
  image_with 17 18
  PG_HEADERS_REQUIRE="18" run 0 "PG_HEADERS_REQUIRE=18 still requires 17 (present)"
  image_with 18
  PG_HEADERS_REQUIRE="18" run 1 "17 is always required"
  rm -rf "$t/out"
  if (CONTAINER="$t/bin/engine" FAKE_IMAGE_ROOT="$t/img" extract "img:latest" "$t/out") >/dev/null 2>&1; then
    echo "FAIL an unpinned image is refused"; fail=1
  else
    echo "ok   an unpinned image is refused"
  fi
  # The real pin file parses.
  PG_HEADERS_PINS= pinned_majors >/dev/null || { echo "FAIL the pin file parses"; fail=1; }
  [ $fail -eq 0 ] && echo "pg-headers self-test: ok"
  return $fail
}

if [ "${1:-}" = --self-test ]; then
  self_test
  exit $?
fi
image=${1:?usage: pg-headers.sh <image@sha256:digest> <out-dir> | --self-test}
out=${2:?usage: pg-headers.sh <image@sha256:digest> <out-dir> | --self-test}
extract "$image" "$out"
