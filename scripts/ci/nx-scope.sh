#!/usr/bin/env bash
# Which Nx command a CI job uses (docs/guides/nx-cloud.md): `affected` on the fast tier, so only
# the projects a change reaches run; `run-many` on the full tier, on a manual dispatch, when the
# base commit is unknown, and on a dev push that writes the Nx Cloud cache (it warms every
# project's entry).
#
# Reads: FULL ('true' on the full tier), EVENT (github.event_name), PR_BASE
# (github.event.pull_request.base.sha), BEFORE (github.event.before), NX_NO_CLOUD.
# Writes to $GITHUB_ENV: NX_CMD, and NX_BASE and NX_HEAD for `affected`.
# The checkout needs history back to the base commit (fetch-depth: 0).
set -euo pipefail

base=''
if [ "${FULL:-false}" != true ] && [ "${EVENT:-}" != workflow_dispatch ]; then
  case "${EVENT:-}" in
    pull_request) base="${PR_BASE:-}" ;;
    push) [ "${NX_NO_CLOUD:-true}" = true ] && base="${BEFORE:-}" ;;
  esac
  # A new branch or a force push has no usable base.
  case "$base" in 0000000000000000000000000000000000000000) base='' ;; esac
  if [ -n "$base" ] && ! git cat-file -e "${base}^{commit}" 2>/dev/null; then
    echo "::notice::base commit $base is not in the checkout; running every project"
    base=''
  fi
fi

out="${GITHUB_ENV:-/dev/stdout}"
if [ -n "$base" ]; then
  {
    echo "NX_CMD=affected"
    echo "NX_BASE=$base"
    echo "NX_HEAD=$(git rev-parse HEAD)"
  } >> "$out"
  echo "nx affected: $base..$(git rev-parse --short HEAD)"
else
  echo "NX_CMD=run-many" >> "$out"
  echo "nx run-many: every selected project"
fi
