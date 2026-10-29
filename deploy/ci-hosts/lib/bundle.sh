# shellcheck shell=bash
# Credentials a worker host needs from the primary, in one tar file (mode 600):
# the CA certificate, a client certificate and key for the worker (signed by
# the primary's CA, whose key never leaves the primary) and the sccache
# settings with the primary's tailnet endpoint.
#
#   primary: sudo setup-host.sh --issue-bundle NAME OUT.tar
#   worker:  sudo setup-host.sh --role worker --bundle OUT.tar ...

issue_bundle() {
  local name=$1 out=$2 tmp
  [[ $ROLE == primary ]] || die "--issue-bundle runs on the primary"
  [[ -s $(owner_cfg)/ca/ca.key ]] || die "no CA on this host: run setup-host.sh (role primary) first"
  [[ -s /etc/loams-ci/sccache-remote.env ]] || die "no sccache-remote.env: is Tailscale up on the primary?"
  tmp=$(mktemp -d)
  chmod 700 "$tmp"
  cp "$TLS_DIR/ca.crt" "$tmp/ca.crt"
  issue_cert "$CI_USER@$name" client "$tmp/client"
  cp /etc/loams-ci/sccache-remote.env "$tmp/sccache-remote.env"
  tailnet_ip >"$tmp/primary"
  (umask 077 && tar -C "$tmp" -cf "$out" ca.crt client.crt client.key sccache-remote.env primary)
  chown "$OWNER_USER:$OWNER_USER" "$out"
  chmod 600 "$out"
  rm -rf "$tmp"
  log "wrote $out for $name (mode 600; copy it to the worker, then delete it)"
}

install_bundle() {
  local bundle=$1 tmp
  [[ -r $bundle ]] || die "cannot read $bundle"
  tmp=$(mktemp -d)
  chmod 700 "$tmp"
  tar -C "$tmp" -xf "$bundle" ca.crt client.crt client.key sccache-remote.env primary || die "not a CI host bundle: $bundle"
  install -d -o root -g "$CI_USER" -m 750 "$TLS_DIR"
  install -o root -g "$CI_USER" -m 644 "$tmp/ca.crt" "$TLS_DIR/ca.crt"
  install -o root -g "$CI_USER" -m 644 "$tmp/client.crt" "$TLS_DIR/client.crt"
  install -o root -g "$CI_USER" -m 640 "$tmp/client.key" "$TLS_DIR/client.key"
  install -o root -g "$CI_USER" -m 640 "$tmp/sccache-remote.env" /etc/loams-ci/sccache-remote.env
  [[ -n $PRIMARY ]] || PRIMARY=$(cat "$tmp/primary")
  # The owner's copy, for Buck2 or sccache from the owner's shell on this host.
  local d
  d="$(owner_cfg)/tls"
  install -d -o "$OWNER_USER" -g "$OWNER_USER" -m 700 "$(owner_cfg)" "$d"
  install -o "$OWNER_USER" -g "$OWNER_USER" -m 644 "$tmp/ca.crt" "$d/ca.crt"
  install -o "$OWNER_USER" -g "$OWNER_USER" -m 644 "$tmp/client.crt" "$d/client.crt"
  install -o "$OWNER_USER" -g "$OWNER_USER" -m 600 "$tmp/client.key" "$d/client.key"
  cat "$tmp/client.crt" "$tmp/client.key" >"$d/client.pem"
  chown "$OWNER_USER:$OWNER_USER" "$d/client.pem"
  chmod 600 "$d/client.pem"
  rm -rf "$tmp"
}
