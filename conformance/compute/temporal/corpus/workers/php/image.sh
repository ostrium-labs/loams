# shellcheck shell=bash
# php_corpus_image <runtime>: prints the PHP corpus worker's image tag
# (localhost/loams-corpus-php:<hash of the Containerfile and its pins>),
# building it from Containerfile when it is missing (TSDK-php). Sourced by
# gateway/legs/php.sh and corpus/generate-php.sh after lib.sh (the pins).
php_corpus_image() {
  local rt=$1 dir tag
  dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  tag=localhost/loams-corpus-php:$(printf '%s\n' "$PHP_IMAGE" "$COMPOSER_IMAGE" "$ROADRUNNER_IMAGE" "$PHP_GRPC_VERSION" |
    cat - "$dir/Containerfile" | sha256sum | cut -c1-16)
  if ! "$rt" image inspect "$tag" >/dev/null 2>&1; then
    "$rt" build -q -t "$tag" -f "$dir/Containerfile" \
      --build-arg "PHP_IMAGE=$PHP_IMAGE" --build-arg "COMPOSER_IMAGE=$COMPOSER_IMAGE" \
      --build-arg "ROADRUNNER_IMAGE=$ROADRUNNER_IMAGE" --build-arg "PHP_GRPC_VERSION=$PHP_GRPC_VERSION" \
      "$dir" >&2
  fi
  echo "$tag"
}
