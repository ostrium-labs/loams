#!/bin/sh
# Download the connector jars named in connector-jars.toml into a directory
# and check each against its sha256 (design 81, D1987). Used by the
# Containerfile's fetch stage; runs with busybox sh/awk and curl.
#
#   fetch-jars.sh <connector-jars.toml> <out-dir> [maven-base-url]
set -eu

manifest=$1
out=$2
base=${3:-https://repo1.maven.org/maven2}
mkdir -p "$out"

# One line per [[jar]]: group artifact version sha256.
awk '
  function flush() { if (a != "") print g, a, v, s; g = a = v = s = "" }
  /^\[\[jar\]\]/ { flush(); injar = 1; next }
  /^\[/ { flush(); injar = 0; next }
  injar && /^[a-z0-9_]+[ \t]*=/ {
    key = $0; sub(/[ \t]*=.*/, "", key)
    val = $0; sub(/^[^=]*=[ \t]*"/, "", val); sub(/".*$/, "", val)
    if (key == "group") g = val
    else if (key == "artifact") a = val
    else if (key == "version") v = val
    else if (key == "sha256") s = val
  }
  END { flush() }
' "$manifest" > "$out/.jars"

count=0
while read -r group artifact version sha; do
  case "$sha" in
    *[!0-9a-f]* | "") echo "fetch-jars: bad sha256 for $artifact" >&2; exit 1 ;;
  esac
  [ ${#sha} -eq 64 ] || { echo "fetch-jars: bad sha256 length for $artifact" >&2; exit 1; }
  path=$(echo "$group" | tr . /)/$artifact/$version/$artifact-$version.jar
  file="$out/$artifact-$version.jar"
  curl -fsSL --retry 3 --proto '=https' -o "$file" "$base/$path"
  echo "$sha  $file" | sha256sum -c -
  count=$((count + 1))
done < "$out/.jars"
rm -f "$out/.jars"

[ "$count" -gt 0 ] || { echo "fetch-jars: no jars in $manifest" >&2; exit 1; }
echo "fetch-jars: $count jars verified"
