#!/usr/bin/env bash
# Usage: LOAMS_GPG_PRIVATE_KEY=<armored secret key> [LOAMS_GPG_KEY_ID=<id>] [LOAMS_GPG_PASSPHRASE=<pw>] \
#        gpg-sign.sh <dir>
# Creates detached ASCII-armored signatures:
#   <file>.sig        for each .AppImage, .deb, .pacman, .pkg.tar.zst, .dmg and .zip in <dir>
#   SHA256SUMS.asc    for <dir>/SHA256SUMS (run checksums.mjs first)
# The .rpm is signed by SignPath and the .exe by SignPath (Authenticode), so neither is touched here.
# With no LOAMS_GPG_PRIVATE_KEY the script warns and exits 0 without writing any signature: the
# release workflow then labels the release unsigned. It never fails to "sign" silently with a key set.
# The key is imported into a throwaway keyring (GNUPGHOME) that is deleted on exit.
set -euo pipefail

dir="${1:-}"
[[ -d "$dir" ]] || { echo "usage: gpg-sign.sh <dir>" >&2; exit 2; }

if [[ -z "${LOAMS_GPG_PRIVATE_KEY:-}" ]]; then
	echo "::warning title=GPG signing skipped::LOAMS_GPG_PRIVATE_KEY is not set; no .sig or SHA256SUMS.asc written, the release is unsigned" >&2
	exit 0
fi
[[ -f "$dir/SHA256SUMS" ]] || { echo "gpg-sign.sh: $dir/SHA256SUMS missing; run checksums.mjs first" >&2; exit 1; }

home="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/loams-gnupg.XXXXXX")"
chmod 700 "$home"
trap 'gpgconf --homedir "$home" --kill all >/dev/null 2>&1 || true; rm -rf "$home"' EXIT
export GNUPGHOME="$home"

pass_args=(--batch --yes --pinentry-mode loopback)
if [[ -n "${LOAMS_GPG_PASSPHRASE:-}" ]]; then
	printf '%s' "$LOAMS_GPG_PASSPHRASE" > "$home/pass"
	chmod 600 "$home/pass"
	pass_args+=(--passphrase-file "$home/pass")
fi

printf '%s\n' "$LOAMS_GPG_PRIVATE_KEY" | gpg "${pass_args[@]}" --import 2>/dev/null \
	|| { echo "gpg-sign.sh: LOAMS_GPG_PRIVATE_KEY could not be imported (is it an armored secret key?)" >&2; exit 1; }

key_args=()
[[ -n "${LOAMS_GPG_KEY_ID:-}" ]] && key_args=(--local-user "$LOAMS_GPG_KEY_ID")

sign() { # <file> <signature>
	gpg "${pass_args[@]}" "${key_args[@]}" --armor --detach-sign --output "$2" "$1"
	echo "signed $(basename "$1")"
}

count=0
shopt -s nullglob
for f in "$dir"/*.AppImage "$dir"/*.deb "$dir"/*.pacman "$dir"/*.pkg.tar.zst "$dir"/*.dmg "$dir"/*.zip; do
	sign "$f" "$f.sig"
	count=$((count + 1))
done
sign "$dir/SHA256SUMS" "$dir/SHA256SUMS.asc"
echo "gpg-sign.sh: $count artifact signature(s) and SHA256SUMS.asc"
