#!/usr/bin/env bash
# Static tests for the k3s home path of deploy/byoc (OP2-C Task 3): the
# generated cloud-init files, the pins, `loams-byoc-secrets --get`, and
# deploy/byoc/lib/loams-k3s-bootstrap run end to end with fakes for tailscale,
# iptables and systemctl on PATH, a local server for the k3s release, the
# install script and GHCR's registry API, and mock_cloud.py for the secret store.
# No cloud account and no network. LOAMS_K3S_SKIP_SYSTEM=1 skips the root check,
# the data disk and the apt installs, and turns on the test hooks.
#
# Needs: bash, jq, curl, tar, python3 with PyYAML, git. CLOUD_INIT_SRC=<a
# cloud-init source tree> adds the cloud-init schema check.
# shellcheck disable=SC2015,SC2154,SC2317,SC2329 # (SC2317/SC2329: the check functions are passed to t) # A && B || C is used on purpose: C reports a failed check
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
byoc="$repo/deploy/byoc"
boot="$byoc/lib/loams-k3s-bootstrap"
secrets="$byoc/lib/loams-byoc-secrets"
tmp="$(mktemp -d "${TMPDIR:-/var/tmp}/byoc-k3s.XXXXXX")"
pids=()
trap 'for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$tmp"' EXIT
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
t() { local name="$1"; shift; if "$@" >"$tmp/log" 2>&1; then ok "$name"; else bad "$name"; sed 's/^/     /' "$tmp/log" | head -20; fi; }
b64() { base64 -w0; }

# --- the generated files -------------------------------------------------------
t "k3s_cloud_init_files_are_generated_and_current" bash -c "
  '$byoc/gen-cloud-init.sh' --check && for c in aws gcp azure; do [ -s \"$byoc/\$c/cloud-init-k3s.yaml\" ] || { echo \"missing \$c/cloud-init-k3s.yaml\"; exit 1; }; done"

pins_ok() {
  local f="$byoc/k3s.env"
  [[ -f "$f" ]] || { echo "no k3s.env"; return 1; }
  # shellcheck disable=SC1090
  ( . "$f"
    [[ "$K3S_VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+\+k3s[0-9]+$ ]] || { echo "K3S_VERSION $K3S_VERSION"; exit 1; }
    for v in K3S_SHA256_AMD64 K3S_SHA256_ARM64 K3S_INSTALL_SHA256; do [[ "${!v}" =~ ^[0-9a-f]{64}$ ]] || { echo "$v is not a sha256"; exit 1; }; done
    [[ "$TAILSCALE_KEY_FPR" =~ ^[0-9A-F]{40}$ ]] || { echo "TAILSCALE_KEY_FPR"; exit 1; }
    [[ "$POSTGRES_IMAGE" =~ ^docker\.io/library/postgres@sha256:[0-9a-f]{64}$ ]] || { echo "POSTGRES_IMAGE $POSTGRES_IMAGE"; exit 1; } )
}
t "k3s_pins_are_official_and_by_digest" pins_ok

# a PyYAML parse of every k3s cloud-init file, as generated and with the module's
# placeholders filled; plus the cloud-init schema when a cloud-init tree is given
fill() {  # file -> stdout, every @@NAME@@ filled with a sample value
  sed -e 's/@@VERSION@@/0.1.4/g' -e 's/@@BUCKET@@/my-bkt/g' -e 's/@@REGION@@/us-east-1/g' -e 's/@@PROJECT@@/my-project/g' \
    -e 's/@@CONTAINER@@/loams/g' -e 's/@@STORAGE_ACCOUNT@@/myacct/g' -e 's/@@VAULT@@/my-vault/g' -e 's/@@CLIENT_ID@@//g' \
    -e 's#@@SECRET_PREFIX@@#loams/t#g' -e 's/@@PROFILES@@//g' -e 's/@@HOSTNAME@@/loams/g' -e 's/@@DATA_DEVICE@@//g' \
    -e 's/@@CONFLUENT_INTEGRATIONS@@//g' -e 's/@@TAILNET_DOMAIN@@/tail1234.ts.net/g' -e 's/@@DEPLOYMENT@@/dep1/g' \
    -e 's/@@ACCESS_DB@@/local/g' -e 's/@@HOME_VALUES_B64@@/e30=/g' -e 's/@@HOME_SECRETS_B64@@/W10=/g' \
    -e 's/@@TARGET_FACTS_B64@@/e30=/g' -e 's/@@CHART_DIGEST@@//g' "$1"
}
parses() { python3 -I -c 'import sys,yaml; d=yaml.safe_load(open(sys.argv[1])); assert d["write_files"] and d["runcmd"]' "$1"; }
for c in aws gcp azure; do
  f="$byoc/$c/cloud-init-k3s.yaml"
  fill "$f" >"$tmp/$c-k3s.yaml" 2>/dev/null
  t "cloud_init_k3s_parses_$c (template and rendered)" bash -c "
    $(declare -f parses); parses '$f' && parses '$tmp/$c-k3s.yaml' &&
    ! grep -v '^#' '$tmp/$c-k3s.yaml' | grep -qE '@@[A-Z0-9_]+@@'"
  if [[ -n "${CLOUD_INIT_SRC:-}" ]]; then
    t "cloud_init_k3s_validates_$c (template)" bash -c "cd '$CLOUD_INIT_SRC' && python3 -m cloudinit.cmd.main schema --config-file '$f' 2>&1 | grep -q '^Valid schema'"
    t "cloud_init_k3s_validates_$c (rendered)" bash -c "cd '$CLOUD_INIT_SRC' && python3 -m cloudinit.cmd.main schema --config-file '$tmp/$c-k3s.yaml' 2>&1 | grep -q '^Valid schema'"
  fi
done
[[ -n "${CLOUD_INIT_SRC:-}" ]] || echo "skip cloud_init_k3s_validates (CLOUD_INIT_SRC is not set)"

# the files in the cloud-init are the files in lib/
for c in aws gcp azure; do
  w() { python3 -I "$here/write_file.py" "$byoc/$c/cloud-init-k3s.yaml" "$1"; }
  t "k3s_scripts_in_${c}_match_lib" bash -c "
    $(declare -f w); export byoc='$byoc' c='$c' here='$here'
    diff <(w /usr/local/bin/loams-k3s-bootstrap) '$byoc/lib/loams-k3s-bootstrap' &&
    diff <(w /usr/local/bin/loams-byoc-secrets) '$byoc/lib/loams-byoc-secrets' &&
    diff <(w /etc/systemd/system/loams-k3s.service) '$byoc/lib/loams-k3s.service' &&
    diff <(w /etc/systemd/system/loams-k3s-imds-block.service) '$byoc/lib/loams-k3s-imds-block.service' &&
    diff <(w /etc/loams-k3s/pins.env) '$byoc/k3s.env' &&
    diff <(w /etc/loams-k3s/access-db.yaml.tmpl) '$byoc/k3s/access-db.yaml.tmpl' &&
    w /etc/loams-k3s/config.env | grep -q '^TAILNET_DOMAIN=@@TAILNET_DOMAIN@@\$' &&
    w /etc/loams-k3s/config.env | grep -q '^HOME_SECRETS_B64=@@HOME_SECRETS_B64@@\$' &&
    w /etc/loams-k3s/config.env | grep -q '^CHART_DIGEST=@@CHART_DIGEST@@\$' &&
    ! grep -q 'docker-ce' '$byoc/$c/cloud-init-k3s.yaml'"
done

# --- the secret store mock and the loams-byoc-secrets --get option ------------------
ca_json="$(jq -cn '{server_ca_crt:"-----BEGIN CERTIFICATE-----\nSERVERCRT-PLANTED\n-----END CERTIFICATE-----\n",
  server_ca_key:"-----BEGIN EC PRIVATE KEY-----\nSERVERKEY-PLANTED\n-----END EC PRIVATE KEY-----\n",
  client_ca_crt:"-----BEGIN CERTIFICATE-----\nCLIENTCRT-PLANTED\n-----END CERTIFICATE-----\n",
  client_ca_key:"-----BEGIN EC PRIVATE KEY-----\nCLIENTKEY-PLANTED\n-----END EC PRIVATE KEY-----\n"}')"
ext_dsn='postgres://app:ext-PLANTED@db.example.com:5432/app?sslmode=require'
jq -n --arg ca "$ca_json" --arg dsn "$ext_dsn" '{
  aws: {"loams/t/tailnet-authkey": "tskey-PLANTED-1", "loams/t/k3s-ca": $ca,
        "loams/t/access-db": ({password:"PwPLANTED0123456789abcdef"}|tojson),
        "loams/x/tailnet-authkey": "tskey-PLANTED-1", "loams/x/k3s-ca": $ca,
        "loams/x/access-db": ({dsn:$dsn}|tojson)},
  gcp: {"mock-project/loams-t-tailnet-authkey": "tskey-PLANTED-1"},
  azure: {"loams-t-tailnet-authkey": "tskey-PLANTED-1"}}' >"$tmp/secrets.json"
mkdir -p "$tmp/srv" "$tmp/t"
python3 -I "$here/mock_cloud.py" "$tmp/mock.port" "$tmp/secrets.json" &
pids+=("$!")
python3 -I "$here/serve_dir.py" "$tmp/srv" "$tmp/srv.port" 2>/dev/null &
pids+=("$!")
for _ in $(seq 50); do [[ -s "$tmp/mock.port" && -s "$tmp/srv.port" ]] && break; sleep 0.1; done
mock="http://127.0.0.1:$(cat "$tmp/mock.port")"
web="http://127.0.0.1:$(cat "$tmp/srv.port")"

cat >"$tmp/get-aws.env" <<CFG
LOAMS_BYOC_CLOUD=aws
SECRET_PREFIX=loams/t
REGION=us-east-1
CFG
cat >"$tmp/get-gcp.env" <<CFG
LOAMS_BYOC_CLOUD=gcp
SECRET_PREFIX=loams-t
CFG
cat >"$tmp/get-azure.env" <<CFG
LOAMS_BYOC_CLOUD=azure
SECRET_PREFIX=loams-t
AZURE_VAULT=mock-vault
CFG
get() {  # cloud name -> stdout of --get; stderr to $tmp/get.err
  local c="$1" sm=sm; [[ "$c" == azure ]] && sm=kv
  env LOAMS_BYOC_CONFIG="$tmp/get-$c.env" LOAMS_BYOC_IMDS_URL="$mock/$c" LOAMS_BYOC_METADATA_URL="$mock/$c" \
    LOAMS_BYOC_SECRETS_URL="$mock/$c-$sm" "$secrets" --get "$2" "$c" 2>"$tmp/get.err"
}
secrets_get_checks() {
  local c
  for c in aws gcp azure; do
    cmp -s <(get "$c" tailnet-authkey) <(printf '%s' tskey-PLANTED-1) || { echo "$c: not exactly the value: $(get "$c" tailnet-authkey | od -c | head -3)"; return 1; }
    [[ ! -s "$tmp/get.err" ]] || { echo "$c: stderr not empty: $(cat "$tmp/get.err")"; return 1; }
  done
  cmp -s <(get aws k3s-ca) <(printf '%s' "$ca_json") || { echo "aws: multi-line JSON value differs"; return 1; }
  local out
  out="$(get aws no-such-secret)" && { echo "a missing secret succeeded"; return 1; }
  [[ -z "$out" ]] || { echo "stdout for a missing secret: $out"; return 1; }
  grep -q 'missing secret loams/t/no-such-secret' "$tmp/get.err" || { echo "stderr: $(cat "$tmp/get.err")"; return 1; }
  ! get aws 'a b;c' >/dev/null 2>&1 || { echo "a bad name was accepted"; return 1; }
}
t "secrets_get_prints_only_value" secrets_get_checks

# --- compose is unchanged, except the secrets script ------------------------------------
compose_unchanged() {
  local base; base="$(git -C "$repo" merge-base HEAD origin/main 2>/dev/null)" || base=HEAD
  for c in aws gcp azure; do
    git -C "$repo" show "$base:deploy/byoc/$c/cloud-init.yaml" >"$tmp/base-$c.yaml" || { echo "no $c/cloud-init.yaml at $base"; return 1; }
    python3 -I - "$tmp/base-$c.yaml" "$byoc/$c/cloud-init.yaml" <<'PY' || return 1
import difflib, sys, yaml
a, b = (yaml.safe_load(open(p)) for p in sys.argv[1:3])
SEC = "/usr/local/bin/loams-byoc-secrets"
fa = {f["path"]: f for f in a.pop("write_files")}
fb = {f["path"]: f for f in b.pop("write_files")}
assert a == b, "cloud-init keys other than write_files changed"
assert list(fa) == list(fb), "the set or order of write_files changed"
for p in fa:
    if p == SEC:
        continue
    assert fa[p] == fb[p], f"{p} changed"
assert {k: v for k, v in fa[SEC].items() if k != "content"} == {k: v for k, v in fb[SEC].items() if k != "content"}
ops = difflib.SequenceMatcher(None, fa[SEC]["content"].splitlines(), fb[SEC]["content"].splitlines(), autojunk=False).get_opcodes()
bad = [o for o in ops if o[0] not in ("equal", "insert")]
assert not bad, f"{SEC} lost or rewrote lines: {bad}"
PY
  done
}
t "compose_cloud_init_unchanged_except_secrets_get" compose_unchanged

# --- the bootstrap, end to end against fakes ---------------------------------------------
mkdir -p "$tmp/bin" "$tmp/t"
cat >"$tmp/bin/tailscale" <<'SH'
#!/usr/bin/env bash
log="${LOAMS_TEST_LOG:?}"
echo "$*" >>"$log/tailscale.log"
case "$1" in
  ip) [[ -e "$log/ts-up" ]] && echo 100.64.0.1 || exit 1 ;;
  up)
    for a in "$@"; do case "$a" in file:*) cat "${a#file:}" >"$log/ts-key" ;; esac; done
    touch "$log/ts-up" ;;
esac
SH
cat >"$tmp/bin/iptables" <<'PY'
#!/usr/bin/env python3
# state file lines, in order: "chain <table> <name>" and "rule <table> <chain> <args>";
# -I <chain> <n> inserts as the n-th rule of that table and chain
import os, sys
st = os.environ["LOAMS_TEST_LOG"] + "/ipt"
a = sys.argv[1:]
if a[0] == "-w":
    a = a[1:]
table = "filter"
if a[0] == "-t":
    table, a = a[1], a[2:]
op, chain, rest = a[0], a[1], a[2:]
lines = open(st).read().splitlines() if os.path.exists(st) else []
def save():
    open(st, "w").write("".join(l + "\n" for l in lines))
prefix = f"rule {table} {chain} "
idx = [i for i, l in enumerate(lines) if l.startswith(prefix)]
if op == "-N":
    if f"chain {table} {chain}" in lines:
        sys.exit("iptables: Chain already exists.")
    lines.append(f"chain {table} {chain}")
elif op == "-C":
    sys.exit(0 if prefix + " ".join(rest) in lines else 1)
elif op == "-D":
    if prefix + " ".join(rest) not in lines:
        sys.exit(1)
    lines.remove(prefix + " ".join(rest))
elif op in ("-I", "-A"):
    pos = None
    if rest and rest[0].isdigit():
        pos, rest = int(rest[0]), rest[1:]
    new = prefix + " ".join(rest)
    if op == "-A" or not idx or pos is None or pos > len(idx):
        at = (idx[-1] + 1) if idx else len(lines)
    else:
        at = idx[pos - 1]
    lines.insert(at, new)
else:
    sys.exit(f"fake iptables: {op}")
save()
PY
cat >"$tmp/bin/systemctl" <<'SH'
#!/usr/bin/env bash
log="${LOAMS_TEST_LOG:?}"
echo "$*" >>"$log/systemctl.log"
exit 0
SH
chmod +x "$tmp/bin/"*

# a local k3s release, install.sh and GHCR
tag='v9.9.9+k3s1'; rel=9.9.9
srvd="$tmp/srv"
mkdir -p "$srvd/k3s/$tag" "$srvd/raw/$tag" "$srvd/ghcr"
printf 'fake-k3s-amd64\n' >"$srvd/k3s/$tag/k3s"
printf 'fake-k3s-arm64\n' >"$srvd/k3s/$tag/k3s-arm64"
cat >"$srvd/raw/$tag/install.sh" <<'SH'
#!/bin/sh
r="$LOAMS_K3S_ROOT"
{
  echo "skip_download=$INSTALL_K3S_SKIP_DOWNLOAD"
  echo "exec=$INSTALL_K3S_EXEC"
  if [ -f "$r/var/lib/rancher/k3s/server/tls/server-ca.crt" ] && [ -f "$r/var/lib/rancher/k3s/server/tls/client-ca.key" ]; then echo ca_present=yes; else echo ca_present=no; fi
  if [ -x "$r/usr/local/bin/k3s" ]; then echo k3s_present=yes; else echo k3s_present=no; fi
  if grep -qx 'enable --now loams-k3s-imds-block.service' "$LOAMS_TEST_LOG/systemctl.log" 2>/dev/null; then echo imds_enabled=yes; else echo imds_enabled=no; fi
  if [ -f "$r/var/lib/rancher/k3s/server/psa.yaml" ]; then echo psa_present=yes; else echo psa_present=no; fi
  if [ -e "$r/var/lib/rancher/k3s/server/manifests/loams-home.yaml" ]; then echo manifest_present=yes; else echo manifest_present=no; fi
} >>"$LOAMS_TEST_LOG/install.log"
SH
sha() { sha256sum "$1" | cut -d' ' -f1; }
cat >"$tmp/pins.env" <<PINS
K3S_VERSION=$tag
K3S_SHA256_AMD64=$(sha "$srvd/k3s/$tag/k3s")
K3S_SHA256_ARM64=$(sha "$srvd/k3s/$tag/k3s-arm64")
K3S_INSTALL_SHA256=$(sha "$srvd/raw/$tag/install.sh")
$(grep -E '^(TAILSCALE_KEY_FPR|POSTGRES_IMAGE)=' "$byoc/k3s.env")
PINS
pg_image="$(sed -n 's/^POSTGRES_IMAGE=//p' "$byoc/k3s.env")"

ghcr="$srvd/ghcr/v2/ostrium-labs/charts/loams-operator"
mkdir -p "$ghcr/manifests" "$ghcr/blobs" "$tmp/chart/loams-operator"
printf 'apiVersion: v2\nname: loams-operator\nversion: %s\n' "$rel" >"$tmp/chart/loams-operator/Chart.yaml"
tar -C "$tmp/chart" -czf "$tmp/chart.tgz" loams-operator
layer="sha256:$(sha "$tmp/chart.tgz")"
printf '{"name":"loams-operator"}' >"$tmp/chart-config.json"
cfgd="sha256:$(sha "$tmp/chart-config.json")"
printf 'prov' >"$tmp/chart.prov"
provd="sha256:$(sha "$tmp/chart.prov")"
cp "$tmp/chart.tgz" "$ghcr/blobs/$layer"; cp "$tmp/chart-config.json" "$ghcr/blobs/$cfgd"; cp "$tmp/chart.prov" "$ghcr/blobs/$provd"
jq -n --arg l "$layer" --arg c "$cfgd" --arg p "$provd" '{schemaVersion:2, mediaType:"application/vnd.oci.image.manifest.v1+json",
  config:{mediaType:"application/vnd.cncf.helm.config.v1+json", digest:$c, size:22},
  layers:[{mediaType:"application/vnd.cncf.helm.chart.provenance.v1.prov", digest:$p, size:4},
          {mediaType:"application/vnd.cncf.helm.chart.content.v1.tar+gzip", digest:$l, size:1}]}' >"$ghcr/manifests/$rel"
printf '{"token":"anon-token"}' >"$srvd/ghcr/token"
mdigest="sha256:$(sha "$ghcr/manifests/$rel")"

home_spec='{"base":{"access":{"identity":{"database":{"secretRef":"loams-access-identity-db"}},"openfga":{"database":{"secretRef":"loams-access-openfga-db"}}}}}'
home_values="$(jq -cn --argjson s "$home_spec" '{replicas:1, home:{create:true, spec:$s}}')"
home_secrets='[{"name":"loams-access-identity-db","key":"dsn","database":"loams_access_identity"},{"name":"loams-access-openfga-db","key":"dsn","database":"loams_access_openfga"}]'
facts='{"deployment":"dep1","target":"aws-k3s","cloud":"aws","region":"us-east-1","bucket_url":"s3://my-bkt/loams","workload_identity":"role-arn"}'

mkconfig() {  # file prefix access_db [CHART_DIGEST] [extra line]
  cat >"$1" <<CFG
LOAMS_BYOC_CLOUD=aws
LOAMS_VERSION=$rel
LOAMS_BUCKET_URL=s3://my-bkt/loams
REGION=us-east-1
SECRET_PREFIX=$2
LOAMS_PROFILES=
TS_HOSTNAME=loams-t
DATA_DEVICE=
CONFLUENT_INTEGRATIONS=
TAILNET_DOMAIN=tail1234.ts.net
DEPLOYMENT=dep1
ACCESS_DB=$3
HOME_VALUES_B64=$(printf '%s' "$home_values" | b64)
HOME_SECRETS_B64=$(printf '%s' "$home_secrets" | b64)
TARGET_FACTS_B64=$(printf '%s' "$facts" | b64)
CHART_DIGEST=${4:-}
${5:-}
CFG
}
scene() {  # name -> fresh root and log dirs
  rm -rf "$tmp/root-$1" "$tmp/log-$1"; mkdir -p "$tmp/root-$1" "$tmp/log-$1"
}
boot_run() {  # name config [env...]
  local n="$1" cfg="$2"; shift 2
  env PATH="$tmp/bin:$PATH" TMPDIR="$tmp/t" LOAMS_TEST_LOG="$tmp/log-$n" LOAMS_K3S_SKIP_SYSTEM=1 LOAMS_K3S_ROOT="$tmp/root-$n" \
    LOAMS_K3S_CONFIG="$cfg" LOAMS_K3S_PINS="$tmp/pins.env" LOAMS_K3S_ACCESS_DB_TMPL="$byoc/k3s/access-db.yaml.tmpl" \
    LOAMS_K3S_RELEASE_URL="$web/k3s" LOAMS_K3S_RAW_URL="$web/raw" LOAMS_K3S_GHCR_URL="$web/ghcr" LOAMS_K3S_ARCH=x86_64 \
    LOAMS_K3S_SECRETS_BIN="$secrets" LOAMS_BYOC_IMDS_URL="$mock/aws" LOAMS_BYOC_SECRETS_URL="$mock/aws-sm" \
    "$@" "$boot" >"$tmp/out-$n" 2>&1
}
r() { echo "$tmp/root-$1"; }
manifest() { echo "$(r "$1")/var/lib/rancher/k3s/server/manifests/loams-home.yaml"; }
manifest_json() {  # name -> JSON array of the manifest's documents
  python3 -I -c 'import sys,yaml,json; json.dump([d for d in yaml.safe_load_all(open(sys.argv[1])) if d], sys.stdout)' "$(manifest "$1")"
}
nothing_installed() {  # name: no manifest, no done marker
  [[ ! -e "$(manifest "$1")" && ! -e "$(r "$1")/var/lib/loams-k3s/done" ]]
}

psa_checks() {  # root log: the Pod Security config file is in place before k3s starts, with the right content
  local f="$1/var/lib/rancher/k3s/server/psa.yaml"
  grep -qx 'psa_present=yes' "$2/install.log" && [[ "$(stat -c %a "$f")" == 600 ]] || { echo "psa.yaml missing before install.sh, or not 0600"; return 1; }
  python3 -I -c '
import sys, yaml
d = yaml.safe_load(open(sys.argv[1]))
assert d["apiVersion"] == "apiserver.config.k8s.io/v1" and d["kind"] == "AdmissionConfiguration", d
(p,) = d["plugins"]
assert p["name"] == "PodSecurity", p
c = p["configuration"]
assert c["apiVersion"] == "pod-security.admission.config.k8s.io/v1" and c["kind"] == "PodSecurityConfiguration", c
dflt = c["defaults"]
assert dflt["enforce"] == "baseline" and dflt["enforce-version"] == "latest", dflt
assert dflt["audit"] == "restricted" and dflt["warn"] == "restricted", dflt
assert sorted(c["exemptions"]["namespaces"]) == ["kube-system", "loams-system"], c["exemptions"]
' "$f"
}

# 1. a good local run: every step, in order
scene local; mkconfig "$tmp/local.env" loams/t local
if boot_run local "$tmp/local.env"; then
  l="$tmp/log-local"; root="$(r local)"
  manifest_json local >"$tmp/local.json"
  [[ -e "$root/var/lib/loams-k3s/done" ]] && ok "step 8: the done marker is written" || bad "no done marker: $(cat "$tmp/out-local")"
  grep -q '^up --authkey file:' "$l/tailscale.log" && grep -q -- '--hostname loams-t' "$l/tailscale.log" && [[ "$(cat "$l/ts-key")" == tskey-PLANTED-1 ]] \
    && ! grep -q PLANTED "$l/tailscale.log" && ok "step 1: tailscale up with the auth key from a file (not argv) and the hostname" || bad "tailscale: $(cat "$l/tailscale.log" 2>&1)"
  tls="$root/var/lib/rancher/k3s/server/tls"
  grep -q SERVERCRT-PLANTED "$tls/server-ca.crt" && grep -q SERVERKEY-PLANTED "$tls/server-ca.key" && grep -q CLIENTCRT-PLANTED "$tls/client-ca.crt" && grep -q CLIENTKEY-PLANTED "$tls/client-ca.key" \
    && [[ "$(stat -c %a "$tls/server-ca.key")$(stat -c %a "$tls/client-ca.key")" == 600600 ]] && ok "step 2: both CAs are in the tls directory, keys mode 600" || bad "CA files"
  grep -qx 'ca_present=yes' "$l/install.log" && grep -qx 'k3s_present=yes' "$l/install.log" && grep -qx 'manifest_present=no' "$l/install.log" && ok "the CAs and the binary are in place before install.sh starts k3s" || bad "order: $(cat "$l/install.log")"
  [[ "$(cat "$root/usr/local/bin/k3s")" == fake-k3s-amd64 ]] && grep -qx 'skip_download=true' "$l/install.log" \
    && grep -qx 'exec=server --disable traefik --secrets-encryption --write-kubeconfig-mode 0600 --tls-san loams-t --tls-san loams-t.tail1234.ts.net --kube-apiserver-arg=admission-control-config-file=/var/lib/rancher/k3s/server/psa.yaml' "$l/install.log" \
    && ok "steps 3 and 4: the checked binary, install.sh with the server flags (the brief's, plus the PSA config file)" || bad "install: $(cat "$l/install.log")"
  grep -qx 'enable --now loams-k3s-imds-block.service' "$l/systemctl.log" && grep -qx 'imds_enabled=yes' "$l/install.log" \
    && ok "step 5: the IMDS block is enabled and started before install.sh starts k3s (no window with unblocked pods)" || bad "imds unit: $(cat "$l/systemctl.log") $(cat "$l/install.log")"
  psa_checks "$root" "$l" && ok "psa_baseline_enforced_with_exemptions" || bad "psa_baseline_enforced_with_exemptions"
  cmp -s "$root/var/lib/rancher/k3s/server/static/charts/loams-operator-$rel.tgz" "$tmp/chart.tgz" && ok "step 6: the chart is in k3s's static charts directory" || bad "chart file"
  ! grep -rq PLANTED "$tmp/out-local" && ok "no secret value in the bootstrap's output" || bad "a secret in the output"
  [[ "$(stat -c %a "$(manifest local)")" == 600 ]] && ok "the home manifest is mode 600 (it holds the DSNs)" || bad "manifest mode"
else
  bad "a good local run failed: $(cat "$tmp/out-local")"; : >"$tmp/local.json"
fi

manifest_checks() {  # name
  local j="$tmp/$1.json"
  jq -e '[.[] | select(.kind=="Namespace" and .metadata.name=="loams-system")] | length == 1' "$j" >/dev/null || { echo "namespace"; return 1; }
  jq -e --arg d "$mdigest" '[.[] | select(.kind=="ConfigMap" and .metadata.name=="loams-target" and .metadata.namespace=="loams-system")][0].data
    | .deployment=="dep1" and .target=="aws-k3s" and .cloud=="aws" and .region=="us-east-1" and .bucket_url=="s3://my-bkt/loams" and .workload_identity=="role-arn" and .chart_digest==$d' "$j" >/dev/null || { echo "loams-target facts"; return 1; }
  jq -e --argjson s "$home_spec" --arg rel "$rel" '[.[] | select(.kind=="HelmChart")] | length == 1 and .[0].apiVersion=="helm.cattle.io/v1" and .[0].metadata.name=="loams-operator" and .[0].metadata.namespace=="kube-system"
    and .[0].spec.chart=="https://%{KUBERNETES_API}%/static/charts/loams-operator-\($rel).tgz" and .[0].spec.targetNamespace=="loams-system"
    and (.[0].spec.valuesContent | fromjson | .home.create == true and .home.spec == $s and .replicas == 1)' "$j" >/dev/null || { echo "HelmChart"; return 1; }
}
t "manifest_has_home_create_true_and_spec" manifest_checks local

local_db_checks() {
  local j="$tmp/local.json"
  jq -e --arg img "$pg_image" '[.[] | select(.kind=="StatefulSet" and .metadata.name=="loams-access-db" and .metadata.namespace=="loams-system")] | length==1
    and .[0].spec.template.spec.containers[0].image == $img and (.[0].spec.template.spec.containers[0].image | test("@sha256:[0-9a-f]{64}$"))
    and .[0].spec.volumeClaimTemplates[0].spec.storageClassName=="local-path" and .[0].spec.volumeClaimTemplates[0].spec.resources.requests.storage=="10Gi"' "$j" >/dev/null || { echo "statefulset"; return 1; }
  [[ "$pg_image" == docker.io/library/postgres@sha256:* ]] || { echo "not the official image: $pg_image"; return 1; }
  jq -e '[.[] | select(.kind=="Service" and .metadata.name=="loams-access-db" and .metadata.namespace=="loams-system")][0].spec.ports[0].port == 5432' "$j" >/dev/null || { echo "service"; return 1; }
  jq -e '[.[] | select(.kind=="ConfigMap" and .metadata.name=="loams-access-db-init")][0].data
    | (to_entries | map(.value) | join("\n")) as $s | ($s | contains("CREATE DATABASE loams_access_identity")) and ($s | contains("CREATE DATABASE loams_access_openfga"))' "$j" >/dev/null || { echo "init databases"; return 1; }
  jq -e '[.[] | select(.kind=="StatefulSet")][0] | (.spec.template.spec.containers[0].env | map(select(.name=="POSTGRES_PASSWORD"))[0].valueFrom.secretKeyRef.name) == "loams-access-db-auth"
    and (tostring | contains("PwPLANTED") | not)' "$j" >/dev/null || { echo "the password must come from a Secret, not the StatefulSet"; return 1; }
  jq -e '[.[] | select(.kind=="Secret" and .metadata.name=="loams-access-db-auth")][0].stringData.password == "PwPLANTED0123456789abcdef"' "$j" >/dev/null || { echo "auth secret"; return 1; }
  jq -e '[.[] | select(.kind=="Secret" and .metadata.name=="loams-access-identity-db")][0].stringData.dsn == "postgres://loams:PwPLANTED0123456789abcdef@loams-access-db.loams-system.svc:5432/loams_access_identity?sslmode=disable"
    and ([.[] | select(.kind=="Secret" and .metadata.name=="loams-access-openfga-db")][0].stringData.dsn == "postgres://loams:PwPLANTED0123456789abcdef@loams-access-db.loams-system.svc:5432/loams_access_openfga?sslmode=disable")
    and all(.[] | select(.kind=="Secret"); .metadata.namespace=="loams-system")' "$j" >/dev/null || { echo "DSN secrets"; return 1; }
}
t "access_db_local_renders_statefulset_with_pinned_digest" local_db_checks

# external access database: only the Secrets, no StatefulSet
scene ext; mkconfig "$tmp/ext.env" loams/x external
if boot_run ext "$tmp/ext.env"; then
  manifest_json ext >"$tmp/ext.json"
  t "access_db_external_renders_no_statefulset" bash -c "
    jq -e '[.[] | select(.kind==\"StatefulSet\" or .kind==\"Service\" or .kind==\"PersistentVolumeClaim\" or (.metadata.name==\"loams-access-db-auth\") or (.metadata.name==\"loams-access-db-init\"))] | length == 0' '$tmp/ext.json' &&
    jq -e --arg dsn '$ext_dsn' '[.[] | select(.kind==\"Secret\")] | map({(.metadata.name): .stringData.dsn}) | add == {\"loams-access-identity-db\": \$dsn, \"loams-access-openfga-db\": \$dsn}' '$tmp/ext.json' &&
    jq -e '[.[] | select(.kind==\"HelmChart\")] | length == 1' '$tmp/ext.json'"
else bad "access_db_external_renders_no_statefulset: the run failed: $(cat "$tmp/out-ext")"; fi

# a different architecture gets the arm64 binary and its hash
scene arm; mkconfig "$tmp/arm.env" loams/t local
if boot_run arm "$tmp/arm.env" LOAMS_K3S_ARCH=aarch64 && [[ "$(cat "$(r arm)/usr/local/bin/k3s")" == fake-k3s-arm64 ]]; then ok "arm64 hosts get k3s-arm64 with its own hash"; else bad "arm64: $(cat "$tmp/out-arm")"; fi

# 2. refusals: nothing is installed after the check fails
scene hash; mkconfig "$tmp/hash.env" loams/t local
printf 'tampered\n' >"$srvd/k3s/$tag/k3s"
if boot_run hash "$tmp/hash.env"; then bad "k3s_binary_hash_mismatch_aborts: a tampered binary was accepted"; else
  grep -q 'k3s.*does not match' "$tmp/out-hash" && [[ ! -e "$tmp/log-hash/install.log" ]] && [[ ! -e "$(r hash)/usr/local/bin/k3s" ]] && nothing_installed hash \
    && ok "k3s_binary_hash_mismatch_aborts" || bad "k3s_binary_hash_mismatch_aborts: $(cat "$tmp/out-hash")"; fi
printf 'fake-k3s-amd64\n' >"$srvd/k3s/$tag/k3s"

scene ish; mkconfig "$tmp/ish.env" loams/t local
cp "$srvd/raw/$tag/install.sh" "$tmp/install.sh.good"; printf '# tampered\n' >>"$srvd/raw/$tag/install.sh"
if boot_run ish "$tmp/ish.env"; then bad "install_sh_hash_mismatch_aborts: a tampered install.sh was run"; else
  grep -q 'install.sh.*does not match' "$tmp/out-ish" && [[ ! -e "$tmp/log-ish/install.log" ]] && ok "install_sh_hash_mismatch_aborts" || bad "install_sh_hash_mismatch_aborts: $(cat "$tmp/out-ish")"; fi
cp "$tmp/install.sh.good" "$srvd/raw/$tag/install.sh"

scene layer; mkconfig "$tmp/layer.env" loams/t local
printf 'tampered' >"$ghcr/blobs/$layer"
if boot_run layer "$tmp/layer.env"; then bad "chart_layer_digest_mismatch_aborts: a tampered chart was accepted"; else
  grep -q 'chart layer.*does not match' "$tmp/out-layer" && nothing_installed layer && [[ ! -e "$(r layer)/var/lib/rancher/k3s/server/static/charts/loams-operator-$rel.tgz" ]] \
    && ok "chart_layer_digest_mismatch_aborts" || bad "chart_layer_digest_mismatch_aborts: $(cat "$tmp/out-layer")"; fi
cp "$tmp/chart.tgz" "$ghcr/blobs/$layer"

# R4: a pinned manifest digest
scene pinbad; mkconfig "$tmp/pinbad.env" loams/t local "sha256:$(printf '0%.0s' $(seq 64))"
if boot_run pinbad "$tmp/pinbad.env"; then bad "chart_manifest_digest_pin_mismatch_aborts: a manifest that is not the pinned one was accepted"; else
  grep -q "chart manifest digest $mdigest does not match the pinned CHART_DIGEST" "$tmp/out-pinbad" && nothing_installed pinbad && [[ ! -e "$(r pinbad)/var/lib/rancher/k3s/server/static/charts/loams-operator-$rel.tgz" ]] \
    && ok "chart_manifest_digest_pin_mismatch_aborts" || bad "chart_manifest_digest_pin_mismatch_aborts: $(cat "$tmp/out-pinbad")"; fi
scene pinok; mkconfig "$tmp/pinok.env" loams/t local "$mdigest"
if boot_run pinok "$tmp/pinok.env" && manifest_json pinok >"$tmp/pinok.json" && manifest_checks pinok; then ok "chart_manifest_digest_pin_match_installs (and records chart_digest)"; else bad "a matching pin was refused: $(cat "$tmp/out-pinok")"; fi
grep -q "not pinned" "$tmp/out-local" && grep -q "$mdigest" "$tmp/out-local" && ok "an empty CHART_DIGEST records the digest and says it is not pinned" || bad "unpinned note: $(cat "$tmp/out-local")"
scene pinfmt; mkconfig "$tmp/pinfmt.env" loams/t local "sha256:XYZ"
if boot_run pinfmt "$tmp/pinfmt.env"; then bad "a malformed CHART_DIGEST was accepted"; else ok "a malformed CHART_DIGEST is refused"; fi

# a missing secret stops everything before any change
scene miss; mkconfig "$tmp/miss.env" loams/other local
if boot_run miss "$tmp/miss.env"; then bad "a missing secret should fail"; else
  grep -q 'loams/other/tailnet-authkey' "$tmp/out-miss" && [[ -z "$(ls "$tmp/log-miss")" ]] && nothing_installed miss && ok "a missing secret fails before tailscale, k3s or any file is touched" || bad "missing secret: $(cat "$tmp/out-miss")"; fi

# a bad database name in the home secrets is refused (it ends up in SQL)
scene baddb; mkconfig "$tmp/baddb.env" loams/t local
home_secrets_save="$home_secrets"; home_secrets='[{"name":"loams-access-identity-db","key":"dsn","database":"x\"; DROP DATABASE y; --"}]'
mkconfig "$tmp/baddb.env" loams/t local; home_secrets="$home_secrets_save"
if boot_run baddb "$tmp/baddb.env"; then bad "a database name with SQL in it was accepted"; else nothing_installed baddb && ok "a database name that is not a plain identifier is refused"; fi

# server-ssh: the secrets are files
scene server; mkconfig "$tmp/server.env" loams/t local "" "LOAMS_K3S_CLOUD=server"
mkdir -p "$(r server)/etc/loams-k3s/secrets"
printf 'tskey-PLANTED-1' >"$(r server)/etc/loams-k3s/secrets/tailnet-authkey"
printf '%s' "$ca_json" >"$(r server)/etc/loams-k3s/secrets/k3s-ca.json"
printf '{"password":"PwPLANTED0123456789abcdef"}' >"$(r server)/etc/loams-k3s/secrets/access-db.json"
if boot_run server "$tmp/server.env" LOAMS_BYOC_IMDS_URL=http://127.0.0.1:9 LOAMS_BYOC_SECRETS_URL=http://127.0.0.1:9; then
  [[ "$(cat "$tmp/log-server/ts-key")" == tskey-PLANTED-1 ]] && grep -q SERVERKEY-PLANTED "$(r server)/var/lib/rancher/k3s/server/tls/server-ca.key" && [[ -e "$(r server)/var/lib/loams-k3s/done" ]] \
    && ok "server-ssh: the bootstrap reads /etc/loams-k3s/secrets/ and never the cloud" || bad "server run"
else bad "the server run failed: $(cat "$tmp/out-server")"; fi

# the IMDS block (Review Focus 5)
imds_checks() {
  local l="$tmp/log-imds"; rm -rf "$l"; mkdir -p "$l"
  local run=(env PATH="$tmp/bin:$PATH" LOAMS_TEST_LOG="$l" "$boot" --imds-block)
  # another rule first in the table, as k3s or a user might leave: ours is inserted at position 1
  printf 'chain raw LOAMS-OTHER\nrule raw PREROUTING -j LOAMS-OTHER\n' >"$l/ipt"
  "${run[@]}" >/dev/null && "${run[@]}" >/dev/null || { echo "the block failed"; return 1; }
  [[ "$(grep -cxF 'rule raw LOAMS-IMDS -s 10.42.0.0/16 -d 169.254.169.254 -j DROP' "$l/ipt")" == 1 ]] || { echo "drop rule: $(cat "$l/ipt")"; return 1; }
  [[ "$(grep -cxF 'rule raw PREROUTING -j LOAMS-IMDS' "$l/ipt")" == 1 ]] || { echo "jump rule: $(cat "$l/ipt")"; return 1; }
  [[ "$(grep '^rule raw PREROUTING ' "$l/ipt" | head -1)" == 'rule raw PREROUTING -j LOAMS-IMDS' ]] || { echo "the jump is not first in raw PREROUTING: $(cat "$l/ipt")"; return 1; }
  grep -qxF 'chain raw LOAMS-IMDS' "$l/ipt" || { echo "no raw chain"; return 1; }
  # not in the filter table, where kube-router's chains can accept traffic ahead of it
  ! grep -q '^rule filter ' "$l/ipt" || { echo "a filter-table rule: $(cat "$l/ipt")"; return 1; }
  local u="$byoc/lib/loams-k3s-imds-block.service"
  grep -qx 'Before=k3s.service' "$u" && ! grep -qE '^(After|PartOf)=.*k3s.service' "$u" && grep -qx 'ExecStart=/usr/local/bin/loams-k3s-bootstrap --imds-block' "$u" \
    && grep -qx 'WantedBy=k3s.service multi-user.target' "$u" && grep -qx 'Type=oneshot' "$u" || { echo "unit"; return 1; }
}
t "imds_block_rule_inserted (raw PREROUTING, first, no duplicates; the unit runs before k3s at every boot)" imds_checks

# 3. a rerun is a no-op: no secret store, no tailscale, no install
rm -rf "$tmp/log-local"; mkdir -p "$tmp/log-local"
before="$(cksum <"$(manifest local)")$(stat -c %i "$(manifest local)")"
if boot_run local "$tmp/local.env" LOAMS_BYOC_IMDS_URL=http://127.0.0.1:9 LOAMS_K3S_RELEASE_URL=http://127.0.0.1:9; then
  [[ -z "$(ls "$tmp/log-local")" && "$(cksum <"$(manifest local)")$(stat -c %i "$(manifest local)")" == "$before" ]] && grep -q 'already' "$tmp/out-local" \
    && ok "rerun_is_noop" || bad "rerun_is_noop: $(ls "$tmp/log-local") $(cat "$tmp/out-local")"
else bad "rerun_is_noop: the rerun failed: $(cat "$tmp/out-local")"; fi

# the unit: oneshot, retried (an Azure secret can arrive after boot)
unit_checks() {
  local u="$byoc/lib/loams-k3s.service"
  grep -qx 'Type=oneshot' "$u" && grep -qx 'Restart=on-failure' "$u" && grep -qx 'RestartSec=30' "$u" && grep -qx 'ExecStart=/usr/local/bin/loams-k3s-bootstrap' "$u" \
    && grep -q '^WantedBy=multi-user.target' "$u"
}
t "k3s_unit_is_oneshot_and_retries" unit_checks

exit "$fail"
