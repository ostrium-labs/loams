# shellcheck shell=bash
# Shared helpers for setup-host.sh and status.sh. Sourced, never executed.

log() { printf '\033[1;34m[ci-hosts]\033[0m %s\n' "$*" >&2; }
warn() { printf '\033[1;33m[ci-hosts] warning:\033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31m[ci-hosts] error:\033[0m %s\n' "$*" >&2
  exit 1
}

require_root() {
  [[ $EUID -eq 0 ]] || die "run as root (sudo $0 ...)"
}

# The distribution family: arch (CachyOS, Arch) or debian (Linux Mint, Ubuntu, Debian).
os_family() {
  local id like
  # shellcheck disable=SC1091
  id=$(. /etc/os-release && echo "${ID:-}")
  # shellcheck disable=SC1091
  like=$(. /etc/os-release && echo "${ID_LIKE:-}")
  case " $id $like " in
    *" arch "* | *" cachyos "*) echo arch ;;
    *" debian "* | *" ubuntu "* | *" linuxmint "*) echo debian ;;
    *) die "unsupported distribution: $id ($like)" ;;
  esac
}

# Installs distribution packages that are missing. Names are per family; pass
# them as arch:NAME or debian:NAME, or a bare name for both.
pkg_install() {
  local fam want=() p
  fam=$(os_family)
  for p in "$@"; do
    case $p in
      arch:*) [[ $fam == arch ]] && want+=("${p#arch:}") ;;
      debian:*) [[ $fam == debian ]] && want+=("${p#debian:}") ;;
      *) want+=("$p") ;;
    esac
  done
  local missing=()
  for p in "${want[@]}"; do
    case $fam in
      arch) pacman -Qq "$p" >/dev/null 2>&1 || missing+=("$p") ;;
      debian) dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' || missing+=("$p") ;;
    esac
  done
  ((${#missing[@]})) || return 0
  log "installing packages: ${missing[*]}"
  case $fam in
    arch) pacman -S --noconfirm --needed "${missing[@]}" ;;
    debian)
      DEBIAN_FRONTEND=noninteractive apt-get update -qq
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${missing[@]}"
      ;;
  esac
}

# fetch_verified URL SHA256 DEST: downloads URL to DEST and checks its digest.
# An existing DEST with the right digest is kept (idempotent re-runs).
fetch_verified() {
  local url=$1 sha=$2 dest=$3
  if [[ -f $dest ]] && echo "$sha  $dest" | sha256sum -c --status; then
    return 0
  fi
  log "downloading $url"
  curl -fsSL --retry 3 -o "$dest.part" "$url" || die "download failed: $url"
  if ! echo "$sha  $dest.part" | sha256sum -c --status; then
    rm -f "$dest.part"
    die "checksum mismatch for $url (expected $sha)"
  fi
  mv "$dest.part" "$dest"
}

# The host's Tailscale IPv4 address, or nothing when Tailscale is not up.
tailnet_ip() {
  command -v tailscale >/dev/null 2>&1 || return 0
  tailscale ip -4 2>/dev/null | head -n1
}

# The host's MagicDNS name (host.tailnet.ts.net), or nothing.
tailnet_fqdn() {
  command -v tailscale >/dev/null 2>&1 || return 0
  tailscale status --json --peers=false 2>/dev/null | jq -r '.Self.DNSName // empty' | sed 's/\.$//'
}

# Opens a TCP port on the tailscale0 interface only, for the tailnet address,
# when ufw is active. Other firewalls are left alone (and reported).
open_tailnet_port() {
  local port=$1 ip=$2
  [[ -n $ip ]] || return 0
  if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
    ufw allow in on tailscale0 to "$ip" port "$port" proto tcp comment 'loams local CI (tailnet only)' >/dev/null
  elif systemctl is-active -q firewalld 2>/dev/null; then
    warn "firewalld is active: allow tcp/$port on tailscale0 yourself"
  fi
}

# A random secret, 32 bytes as hex. Never logged.
random_secret() { openssl rand -hex 32; }

# Writes stdin to PATH with MODE and OWNER:GROUP, atomically.
write_file() {
  local path=$1 mode=$2 owner=$3 tmp
  tmp=$(mktemp "$path.XXXXXX")
  cat >"$tmp"
  chmod "$mode" "$tmp"
  chown "$owner" "$tmp"
  mv "$tmp" "$path"
}

gib() { echo $(($1 * 1024 * 1024 * 1024)); }

# write_unit NAME < content: installs a systemd system unit when it changed,
# reloads systemd and sets UNITS_CHANGED=1.
# shellcheck disable=SC2034 # read by the component scripts
UNITS_CHANGED=0
write_unit() {
  local path=/etc/systemd/system/$1 new
  new=$(cat)
  if [[ ! -f $path ]] || [[ $(cat "$path") != "$new" ]]; then
    printf '%s\n' "$new" | write_file "$path" 644 root:root
    systemctl daemon-reload
    # shellcheck disable=SC2034 # read by the component scripts
    UNITS_CHANGED=1
  fi
}
