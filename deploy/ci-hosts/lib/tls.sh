# shellcheck shell=bash
# A private CA for the CI hosts. NativeLink's tailnet listeners require a client
# certificate signed by it (mutual TLS): NativeLink has no token auth, and the
# tailnet is shared with other people's devices, so the network alone is not
# enough. The CA key stays with the owner on the primary host
# (~/.config/loams-ci/ca, mode 600); nothing here is ever printed or committed.

TLS_DIR=/etc/loams-ci/tls

owner_cfg() { echo "$OWNER_HOME/.config/loams-ci"; }

ensure_ca() {
  local ca_dir
  ca_dir="$(owner_cfg)/ca"
  install -d -o "$OWNER_USER" -g "$OWNER_USER" -m 700 "$(owner_cfg)" "$ca_dir"
  if [[ ! -s $ca_dir/ca.key ]]; then
    log "creating the local CI certificate authority"
    (umask 077 && openssl ecparam -name prime256v1 -genkey -noout -out "$ca_dir/ca.key")
    openssl req -x509 -new -key "$ca_dir/ca.key" -sha256 -days 3650 \
      -subj "/O=Loams local CI/CN=Loams local CI CA" -out "$ca_dir/ca.crt" 2>/dev/null
  fi
  chown "$OWNER_USER:$OWNER_USER" "$ca_dir/ca.key" "$ca_dir/ca.crt"
  chmod 600 "$ca_dir/ca.key"
  chmod 644 "$ca_dir/ca.crt"
  install -d -o root -g "$CI_USER" -m 750 "$TLS_DIR"
  install -o root -g "$CI_USER" -m 644 "$ca_dir/ca.crt" "$TLS_DIR/ca.crt"
}

# issue_cert NAME KIND OUT_PREFIX [SAN]: signs a server or client certificate
# with the CA. KIND is server or client. Writes OUT_PREFIX.crt and .key (0600).
issue_cert() {
  local name=$1 kind=$2 out=$3 san=${4:-} ca_dir ext
  ca_dir="$(owner_cfg)/ca"
  ext=$(mktemp)
  if [[ $kind == server ]]; then
    printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=%s\n' "$san" >"$ext"
  else
    printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=clientAuth\n' >"$ext"
  fi
  (umask 077 && openssl ecparam -name prime256v1 -genkey -noout -out "$out.key")
  openssl req -new -key "$out.key" -subj "/O=Loams local CI/CN=$name" -out "$out.csr" 2>/dev/null
  openssl x509 -req -in "$out.csr" -CA "$ca_dir/ca.crt" -CAkey "$ca_dir/ca.key" -CAcreateserial \
    -days 825 -sha256 -extfile "$ext" -out "$out.crt" 2>/dev/null
  rm -f "$out.csr" "$ext"
  chmod 600 "$out.key"
}

# True when CRT exists, was signed by our CA and has more than 30 days left.
cert_ok() {
  local crt=$1
  [[ -s $crt ]] || return 1
  openssl verify -CAfile "$TLS_DIR/ca.crt" "$crt" >/dev/null 2>&1 || return 1
  openssl x509 -checkend $((30 * 86400)) -noout -in "$crt" >/dev/null 2>&1
}

# The primary's server certificate, for loopback, its tailnet IP and its MagicDNS name.
ensure_server_cert() {
  local ip fqdn host san
  ip=$(tailnet_ip)
  fqdn=$(tailnet_fqdn)
  host=$(hostname -s)
  san="DNS:localhost,IP:127.0.0.1,DNS:$host"
  [[ -n $ip ]] && san+=",IP:$ip"
  [[ -n $fqdn ]] && san+=",DNS:$fqdn,DNS:${fqdn%%.*}"
  # Re-issue when the SAN set changed (e.g. Tailscale came up after the first run).
  if cert_ok "$TLS_DIR/server.crt" && [[ -f $TLS_DIR/server.san ]] && [[ $(cat "$TLS_DIR/server.san") == "$san" ]]; then
    return 0
  fi
  log "issuing the server certificate ($san)"
  issue_cert "$host" server "$TLS_DIR/server" "$san"
  echo "$san" >"$TLS_DIR/server.san"
  chown root:"$CI_USER" "$TLS_DIR/server.crt" "$TLS_DIR/server.key" "$TLS_DIR/server.san"
  chmod 640 "$TLS_DIR/server.key"
  chmod 644 "$TLS_DIR/server.crt" "$TLS_DIR/server.san"
}

# A client certificate for the owner (Buck2, Bazel, grpcurl from the owner's
# shell): ~/.config/loams-ci/tls/client.{crt,key} and client.pem (cert + key in
# one file, the form Buck2's tls_client_cert wants).
ensure_owner_client_cert() {
  local d
  d="$(owner_cfg)/tls"
  install -d -o "$OWNER_USER" -g "$OWNER_USER" -m 700 "$d"
  install -o "$OWNER_USER" -g "$OWNER_USER" -m 644 "$TLS_DIR/ca.crt" "$d/ca.crt"
  if ! cert_ok "$d/client.crt"; then
    log "issuing the owner's client certificate"
    issue_cert "$OWNER_USER@$(hostname -s)" client "$d/client"
  fi
  cat "$d/client.crt" "$d/client.key" >"$d/client.pem"
  chown "$OWNER_USER:$OWNER_USER" "$d/client.crt" "$d/client.key" "$d/client.pem"
  chmod 600 "$d/client.key" "$d/client.pem"
}

# The ci user's own client certificate (a worker or runner on this host that
# talks to the primary over the tailnet).
ensure_ci_client_cert() {
  if ! cert_ok "$TLS_DIR/client.crt"; then
    log "issuing the ci client certificate"
    issue_cert "$CI_USER@$(hostname -s)" client "$TLS_DIR/client"
  fi
  chown root:"$CI_USER" "$TLS_DIR/client.crt" "$TLS_DIR/client.key"
  chmod 640 "$TLS_DIR/client.key"
}
