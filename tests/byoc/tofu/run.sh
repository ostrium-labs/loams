#!/usr/bin/env bash
# Validates deploy/tofu without a cloud account: fmt, validate, the offline
# `tofu test` suites (mock providers) and, if installed, tflint.
#
# Needs OpenTofu >= 1.8 (TOFU=<path> to choose one). Providers come from the
# OpenTofu registry on first run; set TF_PLUGIN_CACHE_DIR to reuse downloads, or
# TOFU_PLUGIN_DIR to a directory laid out as registry.opentofu.org/hashicorp/...
# to use them without any download.
# tflint is optional (TFLINT=<path>).
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tofu_dir="$(cd "$here/../../../deploy/tofu" && pwd)"
tofu="${TOFU:-$(command -v tofu || true)}"
[[ -n "$tofu" ]] || { echo "tofu is not installed (set TOFU=)" >&2; exit 2; }
export TF_IN_AUTOMATION=1
init_args=(-backend=false -input=false)
[[ -z "${TOFU_PLUGIN_DIR:-}" ]] || init_args+=("-plugin-dir=$TOFU_PLUGIN_DIR")
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
run() { local name="$1"; shift; if out="$("$@" 2>&1)"; then ok "$name"; else bad "$name"; echo "$out" | sed 's/^/     /' | head -40; fi; }

run "tofu fmt -check" "$tofu" fmt -check -recursive "$tofu_dir"
for ex in aws-vm gcp-vm azure-vm confluent aws-vm-confluent aws-free gcp-free azure-free; do
  run "init $ex" "$tofu" -chdir="$tofu_dir/examples/$ex" init "${init_args[@]}"
  run "validate $ex" "$tofu" -chdir="$tofu_dir/examples/$ex" validate
done
for mod in aws-vm gcp-vm azure-vm confluent aws-free gcp-free azure-free; do
  run "init $mod" "$tofu" -chdir="$tofu_dir/modules/$mod" init "${init_args[@]}"
  run "test $mod" "$tofu" -chdir="$tofu_dir/modules/$mod" test
done

# the confluent module's tables are current (scripts/byoc/confluent-tables.sh gen)
run "confluent module tables are current" "$here/../../../scripts/byoc/confluent-tables.sh" check
# a root that can put a Confluent key in state enforces state and plan encryption (D1827)
run "example_with_keys_enforces_encryption" python3 -I "$here/check_encryption.py" "$tofu_dir/examples"

# no provider source and no state in the tree
run "no state or lock noise committed" bash -c "! git -C '$tofu_dir' ls-files | grep -E '\\.tfstate|\\.terraform/|crash\\.log'"
run "no HashiCorp-licensed (BSL) code: only provider dependencies" bash -c "! grep -rE 'source *= *\"(hashicorp/(terraform|vault|consul|nomad|packer))' '$tofu_dir' --include=*.tf"

tflint="${TFLINT:-$(command -v tflint || true)}"
if [[ -n "$tflint" ]]; then
  ( cd "$tofu_dir" && "$tflint" --init >/dev/null 2>&1 )
  run "tflint" bash -c "cd '$tofu_dir' && '$tflint' --recursive --format compact"
else
  echo "skip tflint (not installed)"
fi
exit "$fail"
