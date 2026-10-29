#!/usr/bin/env bash
# Sets up another laptop as a Loams local CI worker, from the primary, in one command:
#
#   deploy/ci-hosts/provision-remote.sh USER@HOST [--name loams-ci-b] [--tailscale-authkey-file FILE]
#                                       [--gh-token-file FILE] [-- extra setup-host.sh options]
#
# --gh-token-file stores a GitHub token on HOST (/etc/loams-ci/github-token, root
# only) so its runner re-registers itself when GitHub drops it; use a fine-grained
# token limited to Administration read/write on ostrium-labs/loams.
#
# Run it as the owner on the primary (laptop A), from the repository root. It
#   1. issues the worker's credentials bundle (sudo here: the CA and the sccache
#      password are not readable without it);
#   2. mints a runner registration token for ostrium-labs/loams (gh, or GH_TOKEN);
#   3. copies deploy/ci-hosts, rust-toolchain.toml, package.json and .nvmrc to
#      ~/loams-ci-setup on HOST, with the bundle and the token (mode 600);
#   4. runs `sudo setup-host.sh --role worker ...` there over ssh (sudo asks for
#      HOST's password), then deletes the bundle, token and auth key copies.
# Safe to re-run: setup-host.sh is idempotent and an already registered runner
# is kept.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
die() {
  echo "provision-remote: $*" >&2
  exit 1
}

[[ $# -ge 1 && $1 != -* ]] || die "usage: $0 USER@HOST [--name NAME] [--tailscale-authkey-file FILE] [-- setup-host options]"
TARGET=$1
shift
NAME=loams-ci-b
TS_KEY=
GH_FILE=
EXTRA=()
while (($#)); do
  case $1 in
    --name) NAME=$2; shift 2 ;;
    --tailscale-authkey-file) TS_KEY=$2; shift 2 ;;
    --gh-token-file) GH_FILE=$2; shift 2 ;;
    --) shift; EXTRA=("$@"); break ;;
    *) die "unknown option: $1" ;;
  esac
done

work=$(mktemp -d "${XDG_RUNTIME_DIR:-$HOME}/loams-ci-provision.XXXXXX")
chmod 700 "$work"
trap 'rm -rf "$work"' EXIT

echo "1/4 issuing the credentials bundle for $NAME"
sudo "$HERE/setup-host.sh" --issue-bundle "$NAME" "$work/bundle.tar"

echo "2/4 minting a runner registration token for ostrium-labs/loams"
(umask 077 && gh api -X POST repos/ostrium-labs/loams/actions/runners/registration-token --jq .token >"$work/reg-token")
[[ -s $work/reg-token ]] || die "no registration token (is gh authenticated, or GH_TOKEN set?)"
[[ -z $TS_KEY ]] || (umask 077 && cp "$TS_KEY" "$work/ts-authkey")
[[ -z $GH_FILE ]] || (umask 077 && tr -d '[:space:]' <"$GH_FILE" >"$work/gh-token")

echo "3/4 copying the setup to $TARGET:~/loams-ci-setup"
files=(deploy/ci-hosts rust-toolchain.toml package.json)
[[ -f $ROOT/.nvmrc ]] && files+=(.nvmrc)
tar -C "$ROOT" -czf "$work/setup.tgz" "${files[@]}"
# shellcheck disable=SC2029 # the remote paths are meant to expand there
ssh "$TARGET" 'umask 077 && rm -rf ~/loams-ci-setup && mkdir -p ~/loams-ci-setup/.secrets && tar -C ~/loams-ci-setup -xzf -' <"$work/setup.tgz"
for f in bundle.tar reg-token ts-authkey gh-token; do
  [[ -f $work/$f ]] || continue
  # shellcheck disable=SC2029 # $f is meant to expand here
  ssh "$TARGET" "umask 077 && cat > ~/loams-ci-setup/.secrets/$f" <"$work/$f"
done

echo "4/4 running setup-host.sh --role worker on $TARGET (sudo will ask for its password)"
remote_args="--role worker --bundle ~/loams-ci-setup/.secrets/bundle.tar --reg-token ostrium-labs/loams=\$HOME/loams-ci-setup/.secrets/reg-token --tailscale-hostname $NAME"
[[ -z $TS_KEY ]] || remote_args+=" --tailscale-authkey-file \$HOME/loams-ci-setup/.secrets/ts-authkey"
[[ -z $GH_FILE ]] || remote_args+=" --gh-token-file \$HOME/loams-ci-setup/.secrets/gh-token --store-gh-token"
for a in "${EXTRA[@]}"; do remote_args+=" $(printf '%q' "$a")"; done
rc=0
# shellcheck disable=SC2029
ssh -t "$TARGET" "sudo ~/loams-ci-setup/deploy/ci-hosts/setup-host.sh $remote_args" || rc=$?
ssh "$TARGET" 'rm -rf ~/loams-ci-setup/.secrets' || true
((rc == 0)) || die "setup-host.sh failed on $TARGET (exit $rc); fix and re-run this command"
echo "done: $TARGET is a local CI worker. Check it with: ssh $TARGET ~/loams-ci-setup/deploy/ci-hosts/status.sh"
