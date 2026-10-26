#!/usr/bin/env bash
# The PHP SDK's split repository, which is what Packagist reads.
#
# Packagist needs composer.json at the root of a repository, and Composer cannot
# parse this repository's `v0.1.0-preview.2` tags (it knows alpha, beta and RC,
# not preview). So each release makes one commit whose tree is exactly sdks/php
# at the release commit, on top of the split repository's previous release, and
# tags it with Composer's spelling of the version (v0.1.0-alpha.2).
#
#   php-split.sh SHA COMPOSER_TAG REMOTE_URL [--push]
#
# Without --push it builds the commit and the tag locally, prints them, and
# checks the archive Composer would download. With --push it pushes the branch
# `main` and the tag to REMOTE_URL. A tag that already exists on the remote with
# the same tree is left alone (a re-run is safe); one with a different tree fails.
set -euo pipefail

sha="${1:?usage: php-split.sh SHA COMPOSER_TAG REMOTE_URL [--push]}"
tag="${2:?usage: php-split.sh SHA COMPOSER_TAG REMOTE_URL [--push]}"
remote="${3:?usage: php-split.sh SHA COMPOSER_TAG REMOTE_URL [--push]}"
push="${4:-}"

tree=$(git rev-parse --verify "$sha:sdks/php")
git rev-parse --verify -q "$sha:sdks/php/composer.json" >/dev/null || { echo "php-split: no sdks/php/composer.json at $sha" >&2; exit 1; }

parent_args=()
if [ "$push" = --push ]; then
  # The tag this script pushes is lightweight, so look for the ref itself
  # (`refs/tags/$tag^{}` only lists annotated tags, and never matched).
  if existing=$(git ls-remote --tags "$remote" "refs/tags/$tag" | cut -f1) && [ -n "$existing" ]; then
    git fetch -q "$remote" "refs/tags/$tag"
    if [ "$(git rev-parse FETCH_HEAD^{tree})" = "$tree" ]; then
      tagged=$(git rev-parse FETCH_HEAD^{commit})
      # v0.1.2: an earlier, non-atomic push landed the tag but not main (a
      # ruleset refused main). Catch main up when it is behind the tag.
      main_head=$(git ls-remote "$remote" refs/heads/main | cut -f1)
      if [ "$main_head" != "$tagged" ]; then
        if [ -n "$main_head" ]; then
          git fetch -q "$remote" main
          git merge-base --is-ancestor FETCH_HEAD "$tagged" \
            || { echo "php-split: main is not an ancestor of $tag; not moving it" >&2; exit 1; }
        fi
        git push "$remote" "$tagged:refs/heads/main"
        echo "php-split: $tag was already there; main caught up to it"
        exit 0
      fi
      echo "php-split: $tag is already on the split repository with this tree; nothing to do"
      exit 0
    fi
    echo "php-split: $tag is on the split repository with a different tree; refusing to move it" >&2
    exit 1
  fi
  if head=$(git ls-remote "$remote" refs/heads/main | cut -f1) && [ -n "$head" ]; then
    git fetch -q "$remote" main
    parent_args=(-p FETCH_HEAD)
  fi
fi

export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-github-actions[bot]}"
export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME" GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
commit=$(git commit-tree "$tree" "${parent_args[@]}" \
  -m "loams-php $tag" \
  -m "Read-only split of sdks/php from https://github.com/ostrium-labs/loams at $sha. Send changes there.")
echo "php-split: $tag -> $commit (tree $tree, from $sha)"

# What `composer require` downloads: the commit's archive, minus export-ignore.
check_dir=$(mktemp -d "${RUNNER_TEMP:-$PWD}/php-split.XXXXXX")
trap 'rm -rf "$check_dir"' EXIT
git archive "$commit" | tar -x -C "$check_dir"
test -f "$check_dir/composer.json" && test -d "$check_dir/src" && test ! -e "$check_dir/tests"
(cd "$check_dir" && composer validate --no-check-lock --strict)
echo "php-split: the archive has $(find "$check_dir" -type f | wc -l) files"

if [ "$push" = --push ]; then
  # Atomic: both refs or neither, so a refused main never leaves the tag behind alone.
  git push --atomic "$remote" "$commit:refs/heads/main" "$commit:refs/tags/$tag"
  echo "php-split: pushed main and $tag"
fi
