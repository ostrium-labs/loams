# shellcheck shell=bash
# GitHub self-hosted runners (labels self-hosted, loams-local, linux, x64), one
# or more per repository, each a rootless podman container (the Ubuntu 24.04
# image in runner/Containerfile) run by a user unit of the ci user:
#   REPO      -> runner <host>-<repo>,    unit loams-runner-<repo>.service
#   REPO:N    -> runners <host>-<repo>-1..N, units loams-runner-<repo>-1..N.service
# Each container has a memory cap and CARGO_BUILD_JOBS sized so that all the
# host's runners together cannot exhaust its RAM (runner_sizing).
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
    --memory "$RUNNER_MEMORY" --memory-swap "$RUNNER_MEMORY_SWAP"
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

# Prints one line per runner instance: NAME REPO UNIT.
runner_instances() {
  local spec repo count short i host
  host=$(host_tag)
  for spec in "${RUNNER_REPOS[@]}"; do
    repo=${spec%%:*}
    short=${repo##*/}
    if [[ $spec == *:* ]]; then
      count=${spec##*:}
      for ((i = 1; i <= count; i++)); do
        echo "$host-$short-$i $repo loams-runner-$short-$i.service"
      done
    else
      echo "$host-$short $repo loams-runner-$short.service"
    fi
  done
}

# Sizes the runners from the host's RAM unless --runner-memory/--runner-jobs say
# otherwise: what is left after RUNNER_RESERVE_GB (desktop, NativeLink, page
# cache) is split evenly over the main repository's runners, rounded down to 0.5 GiB,
# and each runner gets one cargo job per RUNNER_GB_PER_JOB of it. The disk
# floor a job needs is RUNNER_DISK_GB per runner of the main repository.
runner_sizing() {
  local n split mem_kib
  n=$(runner_instances | wc -l)
  ((n > 0)) || return 0
  # RAM is split over the first repository's runners (ostrium-labs/loams, the
  # ones busy with a PR's ~30 jobs); a single runner of another repository
  # (iggy) gets the same cap and is rarely busy at the same time.
  split=$(runner_instances | awk -v r="${RUNNER_REPOS[0]%%:*}" '$2 == r' | wc -l)
  mem_kib=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
  if [[ -z $RUNNER_MEMORY ]]; then
    RUNNER_MEMORY=$(awk -v k="$mem_kib" -v r="$RUNNER_RESERVE_GB" -v n="$split" \
      'BEGIN { g = (k / 1048576 - r) / n; g = int(g * 2) / 2; if (g < 2) g = 0; printf "%dm", g * 1024 }')
    [[ $RUNNER_MEMORY != 0m ]] ||
      die "not enough RAM for $split runners ($((mem_kib / 1048576)) GiB, $RUNNER_RESERVE_GB GiB reserved): use fewer runners"
  fi
  local mib=${RUNNER_MEMORY%[mM]}
  [[ $RUNNER_MEMORY == *[gG] ]] && mib=$((${RUNNER_MEMORY%[gG]} * 1024))
  if [[ -z $RUNNER_JOBS ]]; then
    RUNNER_JOBS=$(awk -v m="$mib" -v g="$RUNNER_GB_PER_JOB" 'BEGIN { j = int(m / 1024 / g); if (j < 1) j = 1; print j }')
  fi
  # Swap, when the host has it, absorbs a link spike instead of an OOM kill.
  if (($(awk '/^SwapTotal:/ {print $2}' /proc/meminfo) > 0)); then
    RUNNER_MEMORY_SWAP="$((mib * 3 / 2))m"
  else
    RUNNER_MEMORY_SWAP=$RUNNER_MEMORY
  fi
  : "${RUNNER_MIN_FREE_GB:=$((RUNNER_DISK_GB * split))}"
  log "runners: $n on this host; each ${RUNNER_MEMORY} memory (swap up to ${RUNNER_MEMORY_SWAP}), $RUNNER_JOBS cargo jobs; jobs need ${RUNNER_MIN_FREE_GB} GiB free"
}

# --dry-run: what setup_runners would do, without doing it.
runner_plan() {
  runner_sizing
  echo "runner plan for $(host_tag) (role $ROLE, data root $DATA_ROOT):"
  local name repo unit d
  while read -r name repo unit; do
    printf '  %-32s %-22s %s\n' "$name" "$repo" "$unit"
  done < <(runner_instances)
  echo "  each: --memory $RUNNER_MEMORY --memory-swap $RUNNER_MEMORY_SWAP, CARGO_BUILD_JOBS=$RUNNER_JOBS"
  echo "  a job needs ${RUNNER_MIN_FREE_GB} GiB free on $DATA_ROOT"
  for d in "$DATA_ROOT"/runners/*/; do
    [[ -d $d ]] || continue
    runner_instances | awk '{print $1}' | grep -qx "$(basename "$d")" || echo "  retire: $(basename "$d")"
  done
}

# Removes runners of this host that are no longer configured (for example the
# unnumbered <host>-loams after a switch to --runners N): stops and deletes the
# unit, deletes the registration on GitHub (with --gh-token-file) and the files.
retire_stale_runners() {
  local unit_dir=$1 keep d name unit repo id
  keep=" $(runner_instances | awk '{print $1}' | tr '\n' ' ') "
  for d in "$DATA_ROOT"/runners/*/; do
    [[ -d $d ]] || continue
    name=$(basename "$d")
    [[ $keep == *" $name "* ]] && continue
    log "retiring runner $name (no longer configured)"
    for unit in "$unit_dir"/loams-runner-*.service; do
      if [[ ! -f $unit ]] || ! grep -q -- "--name loams-runner-$name " "$unit"; then continue; fi
      as_ci systemctl --user disable --now "$(basename "$unit")" >/dev/null 2>&1 || true
      rm -f "$unit"
    done
    as_ci systemctl --user daemon-reload
    repo=$(sed 's/^\xEF\xBB\xBF//' "$d/runner/.runner" 2>/dev/null | jq -r '.gitHubUrl // empty' | sed 's|^https://github.com/||')
    if [[ -n $repo && -n $GH_TOKEN_FILE && -r $GH_TOKEN_FILE ]]; then
      id=$(github_api GET "repos/$repo/actions/runners?per_page=100" | jq -r --arg n "$name" '.runners[] | select(.name == $n) | .id')
      [[ -n $id ]] && github_api DELETE "repos/$repo/actions/runners/$id" >/dev/null &&
        log "deleted the registration of $name on $repo"
    elif [[ -n $repo ]]; then
      warn "$name stays registered on $repo (offline) without --gh-token-file; GitHub removes it after 14 days, or delete it in the repository settings"
    fi
    # Files a job container created under another uid need the ci user's namespace.
    as_ci podman unshare rm -rf "$d" 2>/dev/null || rm -rf "$d"
  done
}

github_api() {
  local method=$1 path=$2
  curl -fsS -X "$method" \
    -H @<(printf 'Authorization: Bearer %s\nAccept: application/vnd.github+json\n' "$(tr -d '[:space:]' <"$GH_TOKEN_FILE")") \
    "https://api.github.com/$path"
}

setup_runners() {
  [[ ${#RUNNER_REPOS[@]} -gt 0 ]] || return 0
  runner_sizing
  write_status_env
  local home unit_dir
  home=/home/$CI_USER
  unit_dir=$home/.config/systemd/user
  install -d -o "$CI_USER" -g "$CI_USER" -m 700 "$home/.config/systemd" "$unit_dir"
  install -d -o "$CI_USER" -g "$CI_USER" -m 750 "$DATA_ROOT/runners" "$DATA_ROOT/cache" \
    "$DATA_ROOT/cache/cargo" "$DATA_ROOT/cache/rustup" "$DATA_ROOT/cache/toolcache"
  build_runner_image
  retire_stale_runners "$unit_dir"
  local repo name unit new changed
  while read -r name repo unit; do
    install_runner_dist "$DATA_ROOT/runners/$name/runner"
    install -d -o "$CI_USER" -g "$CI_USER" -m 750 "$DATA_ROOT/runners/$name/work"
    register_runner "$name" "$repo"
    new=$(runner_unit "$name" "$repo")
    changed=0
    if [[ ! -f $unit_dir/$unit ]] || [[ $(cat "$unit_dir/$unit") != "$new" ]]; then
      printf '%s\n' "$new" | write_file "$unit_dir/$unit" 644 "$CI_USER:$CI_USER"
      changed=1
    fi
    as_ci systemctl --user daemon-reload
    as_ci systemctl --user enable -q "$unit"
    if ((changed)) || ! as_ci systemctl --user is-active -q "$unit"; then
      as_ci systemctl --user restart "$unit"
    fi
    log "runner $name up for $repo (memory cap $RUNNER_MEMORY, $RUNNER_JOBS cargo jobs)"
  done < <(runner_instances)
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
      RUNNER_JOBS RUNNER_MIN_FREE_GB RUNNER_RESERVE_GB RUNNER_DISK_GB RUNNER_GB_PER_JOB \
      OWNER_USER TS_HOSTNAME; do
      printf '%s=%q\n' "$v" "${!v:-}"
    done
    printf 'SAVED_RUNNER_REPOS=%q\n' "${RUNNER_REPOS[*]}"
    echo "SETUP_ENV_VERSION=2"
  } | write_file "$SETUP_ENV" 600 root:root
  write_status_env
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

# What status.sh (no sudo) needs: the data root and the jobs' disk floor.
# /opt/loams-ci, not /etc/loams-ci: that directory is root:ci 0750, so status.sh
# run by the owner could not read it (and fell back to the defaults).
write_status_env() {
  printf 'DATA_ROOT=%s\nMIN_FREE_GB=%s\nRUNNER_MEMORY=%s\nRUNNER_JOBS=%s\n' \
    "$DATA_ROOT" "$RUNNER_MIN_FREE_GB" "$RUNNER_MEMORY" "$RUNNER_JOBS" | write_file /opt/loams-ci/status.env 644 root:root
  rm -f /etc/loams-ci/status.env
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
  runner_sizing >/dev/null 2>&1
  while read -r name repo unit; do
    root=$DATA_ROOT/runners/$name
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
  done < <(runner_instances)
}
