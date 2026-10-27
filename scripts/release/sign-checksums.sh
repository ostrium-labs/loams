#!/usr/bin/env bash
# Usage: LOAMS_GPG_PRIVATE_KEY=<armored secret key> LOAMS_GPG_KEY_ID=<fingerprint> \
#        [LOAMS_GPG_PASSPHRASE=<pw>] [LOAMS_CHECKSUMS_REGENERATE=1] \
#        sign-checksums.sh <dir> <public key file>
#
# Rewrites <dir>/SHA256SUMS over every file in <dir> (except SHA256SUMS and
# SHA256SUMS.asc), after checking that every entry of the SHA256SUMS already in
# <dir> still matches its file, then writes the detached, ASCII-armored signature
# <dir>/SHA256SUMS.asc and verifies it against <public key file> in a fresh keyring.
#
# LOAMS_CHECKSUMS_REGENERATE=1 is for a release whose assets were rebuilt on
# purpose (v0.1.2's Linux packages, rebuilt on Ubuntu 24.04): the entries that no
# longer match are listed, then the list is rewritten over the files as attached.
#
# Unlike apps/desktop-electron/scripts/gpg-sign.sh this fails without a key: a
# release's SHA256SUMS.asc is either produced and verified, or the run is red.
# The key is imported into a throwaway keyring that is deleted on exit.
set -euo pipefail

dir="${1:-}"
pubkey="${2:-}"
[[ -d "$dir" && -f "$pubkey" ]] || { echo "usage: sign-checksums.sh <dir> <public key file>" >&2; exit 2; }
[[ -n "${LOAMS_GPG_PRIVATE_KEY:-}" ]] || { echo "::error::LOAMS_GPG_PRIVATE_KEY is not set; SHA256SUMS cannot be signed" >&2; exit 1; }
[[ -n "${LOAMS_GPG_KEY_ID:-}" ]] || { echo "::error::LOAMS_GPG_KEY_ID is not set" >&2; exit 1; }

# Resolve the key before leaving the caller's directory: the workflow passes a relative path.
pubkey="$(cd "$(dirname "$pubkey")" && pwd)/$(basename "$pubkey")"
cd "$dir"
rm -f SHA256SUMS.asc
if [[ -f SHA256SUMS ]]; then
	# The release's existing checksums (release-github.yml's) must still hold: a
	# changed asset is a problem to stop on, not something to re-hash over,
	# unless the run says the assets were rebuilt.
	if ! changed=$(sha256sum -c --ignore-missing --quiet SHA256SUMS 2>/dev/null); then
		if [[ "${LOAMS_CHECKSUMS_REGENERATE:-}" != 1 ]]; then
			printf '%s\n' "$changed" >&2
			echo "::error::attached files no longer match SHA256SUMS; if they were rebuilt on purpose, run with regenerate" >&2
			exit 1
		fi
		echo "::warning::regenerating SHA256SUMS over rebuilt assets; these entries changed:"
		printf '%s\n' "$changed"
	fi
	rm -f SHA256SUMS
fi
shopt -s nullglob
files=(*)
[[ ${#files[@]} -gt 0 ]] || { echo "::error::no release assets in $dir" >&2; exit 1; }
LC_ALL=C sha256sum -- "${files[@]}" | LC_ALL=C sort -k2 > SHA256SUMS
echo "SHA256SUMS lists ${#files[@]} files"

base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
base="${base%/}"
(( ${#base} <= 60 )) || base=/tmp
sign_home="$(mktemp -d "$base/loams-gnupg.XXXXXX")"
verify_home="$(mktemp -d "$base/loams-gnupg.XXXXXX")"
chmod 700 "$sign_home" "$verify_home"
cleanup() {
	for h in "$sign_home" "$verify_home"; do
		gpgconf --homedir "$h" --kill all >/dev/null 2>&1 || true
		rm -rf "$h"
	done
}
trap cleanup EXIT

pass_args=(--batch --yes --pinentry-mode loopback)
if [[ -n "${LOAMS_GPG_PASSPHRASE:-}" ]]; then
	printf '%s' "$LOAMS_GPG_PASSPHRASE" > "$sign_home/pass"
	chmod 600 "$sign_home/pass"
	pass_args+=(--passphrase-file "$sign_home/pass")
fi
# Status lines go to a file and are printed only on failure; they never include key material.
if ! printf '%s\n' "$LOAMS_GPG_PRIVATE_KEY" | gpg --homedir "$sign_home" "${pass_args[@]}" --status-fd 3 --import 3>"$sign_home/import.status" 2>/dev/null; then
	echo "::error::LOAMS_GPG_PRIVATE_KEY could not be imported (is it an armored secret key?)" >&2
	grep '^\[GNUPG:\]' "$sign_home/import.status" >&2 || true
	exit 1
fi
gpg --homedir "$sign_home" "${pass_args[@]}" --local-user "$LOAMS_GPG_KEY_ID" \
	--armor --detach-sign --output SHA256SUMS.asc SHA256SUMS

# Verify with the published public key only, so a signature by any other key fails here.
gpg --homedir "$verify_home" --batch --quiet --import "$pubkey"
gpg --homedir "$verify_home" --batch --status-fd 1 --verify SHA256SUMS.asc SHA256SUMS > "$verify_home/verify.status" 2>&1 || true
grep -qE "^\[GNUPG:\] VALIDSIG .*${LOAMS_GPG_KEY_ID^^}" "$verify_home/verify.status" || {
	echo "::error::SHA256SUMS.asc does not verify against $pubkey as $LOAMS_GPG_KEY_ID" >&2
	cat "$verify_home/verify.status" >&2
	exit 1
}
echo "SHA256SUMS.asc verifies against $(basename "$pubkey") ($LOAMS_GPG_KEY_ID)"
