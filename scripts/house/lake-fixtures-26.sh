#!/usr/bin/env bash
# Writes `corpus/he1/26/lake`, the House-written lake fixtures the Rust
# engine's dev server reads for corpus he1-26 (plan HE1 Task 26): runs the
# corpus's setup on the oracle (database he1_26_fixtures), exports each
# table's rows and writes them with loams-house-lake in the House's
# encodings. Needs `scripts/house/oracle.sh up`. Commit the result.
#
#   BUILD_WRAPPER=~/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh scripts/house/lake-fixtures-26.sh
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root/fabric"
curl -fsS http://127.0.0.1:28123/ping >/dev/null || { echo "the oracle is not up on 127.0.0.1:28123 (scripts/house/oracle.sh up)" >&2; exit 1; }
${BUILD_WRAPPER:-} cargo test --locked -j "${JOBS:-4}" -p loams-house-engine --test lake -- --ignored regenerate_corpus_26_lake
