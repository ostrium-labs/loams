#!/usr/bin/env bash
# The local CI runner container's entrypoint.
#   configure  registers the runner (token in ACTIONS_RUNNER_INPUT_TOKEN, never argv)
#   run        (default) runs it
set -euo pipefail
: "${CARGO_HOME:?}" "${RUSTUP_HOME:?}" "${RUNNER_DIR:?}" "${RUNNER_WORK:?}" "${RUNNER_TOOL_CACHE:?}"
mkdir -p "$CARGO_HOME" "$RUSTUP_HOME" "$RUNNER_TOOL_CACHE" "$RUNNER_WORK"

# rustup lives in the persistent CARGO_HOME shared by the host's runners; the
# toolchain each job needs comes from the repository's rust-toolchain.toml.
(
  flock 9
  if [[ ! -x $CARGO_HOME/bin/rustup ]]; then
    rustup-init -y --no-modify-path --default-toolchain none --profile minimal
  fi
) 9>"$CARGO_HOME/.loams-init.lock"

cd "$RUNNER_DIR"
case ${1:-run} in
  configure)
    exec ./config.sh --unattended --replace \
      --url "https://github.com/$RUNNER_REPO" \
      --name "$RUNNER_NAME" \
      --labels "$RUNNER_LABELS" \
      --work "$RUNNER_WORK"
    ;;
  run)
    if [[ ! -s .runner ]]; then
      # GitHub dropped the registration (the runner then deletes its credentials).
      # 78 stops systemd's restart loop; loams-ci-runner-heal.timer re-registers.
      echo "runner $RUNNER_NAME is not registered; loams-ci-runner-heal.timer will register it again" >&2
      exit 78
    fi
    exec ./run.sh
    ;;
  *) exec "$@" ;;
esac
