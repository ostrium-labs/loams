# shellcheck shell=bash
# Tailscale: installs it when missing and joins the tailnet with an auth key
# read from a file (tailscale reads the file itself; the key is never in argv
# or printed). MagicDNS and subnet routes stay off so the laptop's own DNS and
# routing are untouched.

setup_tailscale() {
  if ! command -v tailscale >/dev/null 2>&1; then
    case $(os_family) in
      arch) pkg_install tailscale ;;
      debian)
        local codename
        # shellcheck disable=SC1091
        codename=$(. /etc/os-release && echo "${UBUNTU_CODENAME:-${VERSION_CODENAME:-}}")
        [[ -n $codename ]] || die "cannot tell the Ubuntu codename for the Tailscale repository"
        install -d -m 755 /usr/share/keyrings
        curl -fsSL "https://pkgs.tailscale.com/stable/ubuntu/$codename.noarmor.gpg" -o /usr/share/keyrings/tailscale-archive-keyring.gpg
        echo "deb [signed-by=/usr/share/keyrings/tailscale-archive-keyring.gpg] https://pkgs.tailscale.com/stable/ubuntu $codename main" \
          >/etc/apt/sources.list.d/tailscale.list
        pkg_install tailscale
        ;;
    esac
  fi
  systemctl enable -q --now tailscaled
  if [[ -n $(tailnet_ip) ]]; then
    log "Tailscale up: $(tailnet_ip) ($(tailnet_fqdn))"
    return 0
  fi
  [[ -n $TS_AUTHKEY_FILE && -r $TS_AUTHKEY_FILE ]] ||
    die "Tailscale is not connected: pass --tailscale-authkey-file FILE (or run 'sudo tailscale up' yourself)"
  log "joining the tailnet as $TS_HOSTNAME"
  tailscale up --auth-key="file:$TS_AUTHKEY_FILE" --hostname="$TS_HOSTNAME" \
    --accept-dns=false --accept-routes=false --operator="$OWNER_USER" >/dev/null
  local _
  for _ in $(seq 1 30); do [[ -n $(tailnet_ip) ]] && break; sleep 1; done
  [[ -n $(tailnet_ip) ]] || die "tailscale up did not bring an address"
  log "Tailscale up: $(tailnet_ip)"
}
