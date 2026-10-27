#!/usr/bin/env bash
# Make a throwaway CA and leaf for *.drive9.ai (7 days), and start the
# counting proxy on 127.0.0.1:18089. Then, for any ti process to count:
#   export HTTPS_PROXY=http://127.0.0.1:18089 SSL_CERT_FILE=$LC0_ROOT/pki/bundle.pem
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
LC0_ROOT=${LC0_ROOT:-$HOME/lc0-fs}
P=$LC0_ROOT/pki; mkdir -p "$P"; cd "$P"
if [ ! -e leaf.pem ]; then
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 7 \
    -subj "/CN=lc0-fs spike CA" -keyout ca.key -out ca.pem 2>/dev/null
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj "/CN=drive9.ai" \
    -keyout leaf.key -out leaf.csr 2>/dev/null
  printf 'subjectAltName=DNS:*.drive9.ai,DNS:drive9.ai\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' > ext.cnf
  openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 7 \
    -extfile ext.cnf -out leaf.pem 2>/dev/null
  cat /etc/ssl/certs/ca-certificates.crt ca.pem > bundle.pem
fi
nohup node "$here/countproxy.ts" 18089 leaf.pem leaf.key "$LC0_ROOT/proxy.jsonl" > "$LC0_ROOT/proxy.err" 2>&1 &
echo "proxy pid $!; log $LC0_ROOT/proxy.jsonl"
