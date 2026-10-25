# shellcheck shell=bash
# GitHub self-hosted runners (labels self-hosted, loams-local, linux, x64), one
# per repository, each a rootless podman container (the Ubuntu 24.04 image in
# runner/Containerfile) run by a user unit of the ci user:
#   ~ci/.config/systemd/user/loams-runner-<repo>.service
# Work directories, the runner itself and the shared tool, cargo and rustup
# caches live on DATA_ROOT. Registration tokens are minted from the GitHub API
# (or passed in a file) and handed to the runner through an env file, never argv.

RUNNER_LABELS=${RUNNER_LABELS:-loams-local,linux,x64}
RUNNER_HOOKS=/usr/local/lib/loams-runner/hooks

host_tag() {
  local fqdn
  fqdn=$(tailnet_fqdn)
  if [[ -n $fqdn ]]; then echo "${fqdn%%.*}"; else hostname -s; fi
}

runner_image_tag() {
  local ctx=$1
  echo "localhost/loams-ci-runner:$(cat "$ctx/Containerfile" "$ctx/entrypoint.sh" "$ctx"/hooks/*.sh "$HERE/versions.env" | sha256sum | cut -c1-12)"
}

build_runner_image() {
  local ctx=/opt/loams-ci/runner-ctx
  rm -rf "$ctx"
  cp -r "$HERE/runner" "$ctx"
  chmod -R a+rX "$ctx"
  RUNNER_IMAGE=$(runner_image_tag "$ctx")
  if ! as_ci podman image exists "$RUNNER_IMAGE"; then
    log "building $RUNNER_IMAGE (a few minutes, once per change)"
    (cd / && as_ci podman build --quiet --pull=missing \
      --build-arg "UBUNTU_IMAGE=$UBUNTU_IMAGE" \
      --build-arg "RUSTUP_VERSION=$RUSTUP_VERSION" \
      --build-arg "RUSTUP_SHA256=$RUSTUP_SHA256" \
      --build-arg "DOCKER_CLI_VERSION=$DOCKER_CLI_VERSION" \
      --build-arg "DOCKER_APT_FINGERPRINT=$DOCKER_APT_FINGERPRINT" \
      -t "$RUNNER_IMAGE" -f "$ctx/Containerfile" "$ctx" >/dev/null) || die "runner image build failed"
    # Our own superseded runner images only (they are rebuilt from this file); nothing else.
    local old
    for old in $(as_ci podman images --format '{{.Repository}}:{{.Tag}}' --filter reference=localhost/loams-ci-runner); do
      [[ $old == "$RUNNER_IMAGE" ]] || as_ci podman rmi "$old" >/dev/null 2>&1 || true
    done
  fi
}

install_runner_dist() {
  local dir=$1 tarball=/opt/loams-ci/dl/actions-runner-linux-x64-$RUNNER_VERSION.tar.gz
  [[ -x $dir/bin/Runner.Listener ]] && return 0
  install -d -m 755 /opt/loams-ci/dl
  fetch_verified \
    "https://github.com/actions/runner/releases/download/v$RUNNER_VERSION/actions-runner-linux-x64-$RUNNER_VERSION.tar.gz" \
    "$RUNNER_SHA256" "$tarball"
  install -d -o "$CI_USER" -g "$CI_USER" -m 750 "$dir"
  # A runner that updated itself is newer than the pin: keep it.
  if [[ ! -x $dir/bin/Runner.Listener ]]; then
    tar -xzf "$tarball" -C "$dir"
    chown -R "$CI_USER:$CI_USER" "$dir"
  fi
}

# The podman arguments every runner container gets.
runner_podman_args() {
  local name=$1 repo=$2 uid root=$DATA_ROOT/runners/$1 cache=$DATA_ROOT/cache
  uid=$(id -u "$CI_USER")
  local args=(
    --label "loams-ci-runner=$name"
    --network host
    --memory "$RUNNER_MEMORY"
    --ulimit memlock=-1:-1 --ulimit nofile=65536:65536
    --security-opt "seccomp=$SECCOMP_PROFILE"
    -v "$root:$root" -v "$cache:$cache"
    -v /opt/loams-ci/bin:/opt/loams-ci/bin:ro
    -v "/run/user/$uid/podman/podman.sock:/var/run/docker.sock"
    -e RUNNER_ALLOW_RUNASROOT=1
    -e "RUNNER_DIR=$root/runner" -e "RUNNER_WORK=$root/work"
    -e "RUNNER_REPO=$repo" -e "RUNNER_NAME=$name" -e "RUNNER_LABELS=$RUNNER_LABELS"
    -e "RUNNER_TOOL_CACHE=$cache/toolcache" -e "AGENT_TOOLSDIRECTORY=$cache/toolcache"
    -e "CARGO_HOME=$cache/cargo" -e "RUSTUP_HOME=$cache/rustup"
    -e "PATH=$cache/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    -e "CARGO_BUILD_JOBS=$RUNNER_JOBS" -e CARGO_INCREMENTAL=0
    -e "ACTIONS_RUNNER_HOOK_JOB_STARTED=$RUNNER_HOOKS/job-started.sh"
    -e "ACTIONS_RUNNER_HOOK_JOB_COMPLETED=$RUNNER_HOOKS/job-completed.sh"
    -e "LOAMS_CI_MIN_FREE_GB=$RUNNER_MIN_FREE_GB"
    -e "LOAMS_CI_HOST=$(host_tag)"
  )
  [[ -s $SCCACHE_ENV ]] && args+=(--env-file "$SCCACHE_ENV")
  printf '%s\n' "${args[@]}"
}

# Prints a registration token for REPO: from --reg-token REPO=FILE, else minted
# with the GitHub token in --gh-token-file.
registration_token() {
  local repo=$1 spec
  for spec in "${REG_TOKENS[@]}"; do
    if [[ ${spec%%=*} == "$repo" ]]; then
      tr -d '[:space:]' <"${spec#*=}"
      return 0
    fi
  done
  [[ -n $GH_TOKEN_FILE && -r $GH_TOKEN_FILE ]] ||
    die "runner for $repo is not registered: pass --gh-token-file FILE or --reg-token $repo=FILE"
  curl -fsS -X POST \
    -H @<(printf 'Authorization: Bearer %s\nAccept: application/vnd.github+json\n' "$(tr -d '[:space:]' <"$GH_TOKEN_FILE")") \
    "https://api.github.com/repos/$repo/actions/runners/registration-token" | jq -er .token ||
    die "could not mint a registration token for $repo"
}

register_runner() {
  local name=$1 repo=$2 root=$DATA_ROOT/runners/$1
  [[ -s $root/runner/.runner ]] && return 0
  log "registering runner $name for $repo (labels $RUNNER_LABELS)"
  local envf
  envf=$(mktemp)
  chmod 600 "$envf"
  printf 'ACTIONS_RUNNER_INPUT_TOKEN=%s\n' "$(registration_token "$repo")" >"$envf"
  chown "$CI_USER" "$envf"
  local args
  mapfile -t args < <(runner_podman_args "$name" "$repo")
  as_ci podman run --rm "${args[@]}" --name "loams-runner-$name-configure" --env-file "$envf" \
    "$RUNNER_IMAGE" configure >/dev/null || {
    rm -f "$envf"
    die "runner registration failed for $repo"
  }
  rm -f "$envf"
}

runner_unit() {
  local name=$1 repo=$2 podman args a line
  podman=$(command -v podman)
  mapfile -t args < <(runner_podman_args "$name" "$repo")
  line="$podman run --rm --replace --name loams-runner-$name"
  for a in "${args[@]}"; do line+=" $(printf '%q' "$a")"; done
  cat <<EOF
# Generated by deploy/ci-hosts/setup-host.sh.
[Unit]
Description=Loams local CI: GitHub runner $name ($repo)
Documentation=https://github.com/ostrium-labs/loams/blob/dev/docs/guides/local-ci.md
Wants=podman.socket
After=podman.socket

[Service]
ExecStart=$line $RUNNER_IMAGE run
ExecStop=$podman stop -t 60 loams-runner-$name
Restart=always
RestartSec=10
# 78: not registered; loams-ci-runner-heal.timer registers it again and starts it.
RestartPreventExitStatus=78
TimeoutStopSec=90
LimitMEMLOCK=infinity
LimitNOFILE=65536

[Install]
WantedBy=default.target
EOF
}

setup_runners() {
  [[ ${#RUNNER_REPOS[@]} -gt 0 ]] || return 0
  local home unit_dir
  home=/home/$CI_USER
  unit_dir=$home/.config/systemd/user
  install -d -o "$CI_USER" -g "$CI_USER" -m 700 "$home/.config/systemd" "$unit_dir"
  install -d -o "$CI_USER" -g "$CI_USER" -m 750 "$DATA_ROOT/runners" "$DATA_ROOT/cache" \
    "$DATA_ROOT/cache/cargo" "$DATA_ROOT/cache/rustup" "$DATA_ROOT/cache/toolcache"
  build_runner_image
  local repo name new changed
  for repo in "${RUNNER_REPOS[@]}"; do
    name="$(host_tag)-${repo##*/}"
    install_runner_dist "$DATA_ROOT/runners/$name/runner"
    install -d -o "$CI_USER" -g "$CI_USER" -m 750 "$DATA_ROOT/runners/$name/work"
    register_runner "$name" "$repo"
    new=$(runner_unit "$name" "$repo")
    changed=0
    if [[ ! -f $unit_dir/loams-runner-${repo##*/}.service ]] || [[ $(cat "$unit_dir/loams-runner-${repo##*/}.service") != "$new" ]]; then
      printf '%s\n' "$new" | write_file "$unit_dir/loams-runner-${repo##*/}.service" 644 "$CI_USER:$CI_USER"
      changed=1
    fi
    as_ci systemctl --user daemon-reload
    as_ci systemctl --user enable -q "loams-runner-${repo##*/}.service"
    if ((changed)) || ! as_ci systemctl --user is-active -q "loams-runner-${repo##*/}.service"; then
      as_ci systemctl --user restart "loams-runner-${repo##*/}.service"
    fi
    log "runner $name up for $repo (memory cap $RUNNER_MEMORY, $RUNNER_JOBS cargo jobs)"
  done
  setup_runner_heal
}

# Writes the settings of this run to SETUP_ENV (root, 0600), the defaults of the
# next run and of --heal; copies this directory to /opt/loams-ci/setup so the
# heal timer does not depend on the checkout it was run from; records the data
# root for status.sh.
save_setup() {
  local v
  {
    echo "# Generated by deploy/ci-hosts/setup-host.sh: the settings of the last full run."
    for v in ROLE DATA_ROOT PRIMARY NL_GB NL_JOBS SCCACHE_GB GH_TOKEN_FILE RUNNER_MEMORY \
      RUNNER_JOBS RUNNER_MIN_FREE_GB OWNER_USER TS_HOSTNAME; do
      printf '%s=%q\n' "$v" "${!v:-}"
    done
    printf 'SAVED_RUNNER_REPOS=%q\n' "${RUNNER_REPOS[*]}"
  } | write_file "$SETUP_ENV" 600 root:root
  printf 'DATA_ROOT=%s\nMIN_FREE_GB=%s\n' "$DATA_ROOT" "$RUNNER_MIN_FREE_GB" | write_file /etc/loams-ci/status.env 644 root:root
  local root
  root=$(cd "$HERE/../.." && pwd)
  if [[ $root != /opt/loams-ci/setup ]]; then
    rm -rf /opt/loams-ci/setup.new
    install -d -m 755 /opt/loams-ci/setup.new/deploy
    cp -r "$HERE" /opt/loams-ci/setup.new/deploy/ci-hosts
    local f
    for f in rust-toolchain.toml package.json .nvmrc; do
      [[ -f $root/$f ]] && cp "$root/$f" /opt/loams-ci/setup.new/
    done
    chmod -R a+rX,go-w /opt/loams-ci/setup.new
    rm -rf /opt/loams-ci/setup
    mv /opt/loams-ci/setup.new /opt/loams-ci/setup
  fi
}

# Installs loams-ci-runner-heal.timer: every 5 minutes, --heal re-registers a
# runner whose registration GitHub dropped (the runner then deletes its local
# credentials and stops with "Not configured") and restarts a stopped one.
setup_runner_heal() {
  write_unit loams-ci-runner-heal.service <<'EOF2'
# Generated by deploy/ci-hosts/setup-host.sh.
[Unit]
Description=Loams local CI: re-register and restart the GitHub runners
After=network-online.target

[Service]
Type=oneshot
Nice=10
ExecStart=/opt/loams-ci/setup/deploy/ci-hosts/setup-host.sh --heal
EOF2
  write_unit loams-ci-runner-heal.timer <<'EOF2'
# Generated by deploy/ci-hosts/setup-host.sh.
[Unit]
Description=Loams local CI: runner self-healing every 5 minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=5min

[Install]
WantedBy=timers.target
EOF2
  systemctl enable -q --now loams-ci-runner-heal.timer
  [[ -n $GH_TOKEN_FILE ]] ||
    warn "no --gh-token-file: runners GitHub drops cannot be re-registered automatically"
}

heal_runners() {
  local repo name root unit
  for repo in "${RUNNER_REPOS[@]}"; do
    name="$(host_tag)-${repo##*/}"
    root=$DATA_ROOT/runners/$name
    unit="loams-runner-${repo##*/}.service"
    if [[ ! -s $root/runner/.runner ]]; then
      log "runner $name is not registered (GitHub dropped it?): registering again"
      [[ -n $GH_TOKEN_FILE ]] || die "runner $name needs registering and no --gh-token-file was given at setup"
      RUNNER_IMAGE=$(runner_image_tag /opt/loams-ci/runner-ctx)
      as_ci podman image exists "$RUNNER_IMAGE" || build_runner_image
      register_runner "$name" "$repo"
      as_ci systemctl --user reset-failed "$unit" >/dev/null 2>&1 || true
      as_ci systemctl --user restart "$unit"
    elif ! as_ci systemctl --user is-active -q "$unit"; then
      log "runner $name is not running: restarting it"
      as_ci systemctl --user reset-failed "$unit" >/dev/null 2>&1 || true
      as_ci systemctl --user restart "$unit"
    fi
  done
}
