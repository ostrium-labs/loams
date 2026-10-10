#!/usr/bin/env bash
# Self-test for agentd-deps.sh (DD1 Task 4, D782): feeds canned `cargo tree`
# output to its filter and checks the verdict. Runs without cargo.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/agentd-deps.sh"
tmp_root=${RUNNER_TEMP:-${XDG_CACHE_HOME:-$HOME/.cache}}
mkdir -p "$tmp_root"
scratch=$(mktemp -d "$tmp_root/agentd-deps-test.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

failures=0
expect() {
  local want=$1 name=$2 input=$3 got=0
  bash "$script" --filter < "$input" > "$scratch/out" 2>&1 || got=$?
  if [[ $got != "$want" ]]; then
    echo "FAIL $name: exit $got, expected $want" >&2
    sed 's/^/  | /' "$scratch/out" >&2
    failures=$((failures + 1))
  fi
}
expect_output() {
  local name=$1 needle=$2
  if ! grep -qF -- "$needle" "$scratch/out"; then
    echo "FAIL $name: output lacks '$needle'" >&2
    sed 's/^/  | /' "$scratch/out" >&2
    failures=$((failures + 1))
  fi
}

# A headless tree, in the format the script asks cargo for: one `{p}` per line.
cat > "$scratch/clean" <<'EOF'
loams-agentd v0.0.1 (/src/crates/loams-agentd)
anyhow v1.0.100
loams-agentd-sessions v0.0.1 (/src/crates/loams-agentd-sessions)
loro v1.13.9
portable-pty v0.8.1
gethostname v0.5.0
zbus v5.19.0
tokio v1.53.1
gix-glob v0.21.0
salsa v0.17.0
zeroize v1.8.1
cpal-free v0.1.0
webpki-roots v0.26.11
anyhow v1.0.100 (*)
EOF
expect 0 "clean tree" "$scratch/clean"

# One GUI crate deep in the tree must fail and be named.
{ cat "$scratch/clean"; echo "wry v0.50.0"; } > "$scratch/wry"
expect 1 "wry" "$scratch/wry"
expect_output "wry is named" "wry v0.50.0"
expect_output "the decision is cited" "D782"

# Every family the design names (§50 §4.3), plus egui, eframe and iced (PR
# #393 review), as cargo prints it.
for pkg in "gpui v0.2.0" "gpui_macros v0.1.0" "zed-font-kit v0.14.1" \
  "webkit2gtk v2.0.1" "webkit2gtk-sys v2.0.1" "javascriptcore-rs v1.1.2" \
  "soup3 v0.5.0" "soup3-sys v0.5.0" "gtk v0.18.2" "gtk-sys v0.18.2" \
  "gdk v0.18.2" "gdk-pixbuf-sys v0.18.0" "cpal v0.15.3" "alsa-sys v0.3.1" \
  "egui v0.33.0" "egui_extras v0.33.0" "eframe v0.33.0" "iced v0.13.1" \
  "iced_winit v0.13.0" "WRY v0.50.0"; do
  { cat "$scratch/clean"; echo "$pkg"; } > "$scratch/one"
  expect 1 "$pkg" "$scratch/one"
  expect_output "$pkg is named" "$pkg"
done

# No input is not a clean tree: a failed `cargo tree` must not pass the guard.
: > "$scratch/empty"
expect 2 "empty input" "$scratch/empty"

if ((failures)); then
  echo "agentd-deps.test.sh: $failures failure(s)" >&2
  exit 1
fi
echo "agentd-deps.test.sh: GUI crates rejected, a headless tree and its look-alikes accepted, empty input refused"
