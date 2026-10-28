#!/usr/bin/env bash
# install.sh --kafka (plan CC1 Task 8, D1820): the .env keys and connector
# instance files it would write, through --render-env-only (no SSH, no host).
# shellcheck disable=SC2015 # A && B || C reports a failed check
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
inst="$repo/deploy/release/byoc/install.sh"
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
r() { "$inst" --render-env-only "$@" 2>&1; }
refused() { if "$inst" --render-env-only "$@" >/dev/null 2>&1; then return 1; fi; }
bs="SASL_SSL://pkc-abc12.us-east-1.aws.confluent.cloud:9092"

# iggy: the kafka profile only; no connect, no auth change.
o="$(r --kafka iggy)"
grep -qx 'COMPOSE_PROFILES=kafka' <<<"$o" && ! grep -q 'LOAMS_AUTH_MODE\|connect/instances' <<<"$o" \
  && ok "iggy: the kafka profile only" || bad "iggy: $o"

# confluent: the connectors profile, apikey beside tailscale, one file per topic and sink.
o="$(r --kafka confluent --confluent-bootstrap "$bs" --confluent-topics orders,cdc.app.users=users --confluent-sinks events=loams-prod-events --dir prod)"
grep -qx 'COMPOSE_PROFILES=connectors' <<<"$o" && grep -qx 'LOAMS_AUTH_MODE=tailscale,apikey' <<<"$o" \
  && ok "confluent: connectors profile and API keys on" || bad "confluent profile/auth: $o"
[[ "$(grep -c '^# connect/instances/' <<<"$o")" == 3 ]] \
  && grep -qx '# connect/instances/cf-confluent-orders-in.yaml' <<<"$o" \
  && grep -qx '# connect/instances/cf-confluent-cdc-app-users-in.yaml' <<<"$o" \
  && grep -qx '# connect/instances/cf-confluent-loams-prod-events-out.yaml' <<<"$o" \
  && ok "confluent: an instance per topic and per sink" || bad "instances: $o"
grep -q 'brokers: "pkc-abc12.us-east-1.aws.confluent.cloud:9092"' <<<"$o" && ! grep -q 'SASL_SSL://' <<<"$o" \
  && ok "confluent: the bootstrap scheme is stripped (R0.12)" || bad "bootstrap: $o"
grep -q 'group_id: "loams-prod-cf-confluent-orders-in"' <<<"$o" && grep -q 'transactional_id_prefix: "loams-prod-"' <<<"$o" \
  && grep -q 'stream: "users"' <<<"$o" && grep -q 'topics: \["cdc.app.users"\]' <<<"$o" \
  && ok "confluent: groups and transactional ids under the module's prefix" || bad "prefixes: $o"
grep -q 'username_secret: confluent-confluent/username' <<<"$o" && grep -q 'password: confluent-confluent/password' <<<"$o" \
  && grep -q 'rate_limit: { bytes_per_sec: 65536 }' <<<"$o" && ! grep -q '@[A-Z]*@' <<<"$o" \
  && ok "confluent: secret names, the default rate, no placeholder left" || bad "render: $o"

# Every rendered instance file is valid YAML with the connect role's keys.
if python3 -c 'import yaml' 2>/dev/null; then
  python3 -I - "$o" <<'PY' && ok "confluent: instance files parse" || bad "instance YAML"
import sys, yaml
docs, cur = [], None
for line in sys.argv[1].splitlines():
    if line.startswith("# connect/instances/"):
        cur = []; docs.append(cur); continue
    if cur is not None: cur.append(line)
assert len(docs) == 3
for d in docs:
    y = yaml.safe_load("\n".join(d))
    assert {"name", "connector", "profile", "direction", "stream", "config"} <= y.keys(), y
    assert y["connector"] == "kafka" and y["profile"] == "confluent-cloud"
    assert len(y["name"]) <= 63 and all(c.islower() or c.isdigit() or c == "-" for c in y["name"])
PY
else
  echo "skip instance YAML parse (no PyYAML)"
fi

# --cloud: the fetcher's integration in config.env.
o="$(r --cloud aws --bucket s3://b/loams --region us-east-1 --kafka iggy,confluent --confluent-bootstrap "$bs" --confluent-integration main --confluent-topics orders --confluent-rate 1048576)"
grep -qx 'COMPOSE_PROFILES=kafka,connectors' <<<"$o" && grep -qx 'CONFLUENT_INTEGRATIONS=main' <<<"$o" \
  && grep -qx 'CONNECT_NAMESPACE=0' <<<"$o" && grep -q 'confluent-main/password' <<<"$o" \
  && grep -q 'bytes_per_sec: 1048576' <<<"$o" && ok "iggy,confluent with --cloud" || bad "cloud: $o"
o="$(r --cloud aws --bucket s3://b/loams --region us-east-1)"
! grep -q 'CONFLUENT\|LOAMS_AUTH_MODE' <<<"$o" && ok "no --kafka: nothing Confluent" || bad "no kafka: $o"

# iggy_and_confluent_need_both_named, and the refusals.
refused --kafka all && refused --kafka both && refused --kafka kafka \
  && ok "iggy_and_confluent_need_both_named" || bad "a shorthand for both was accepted"
refused --kafka confluent --confluent-topics t && refused --kafka confluent --confluent-bootstrap "$bs" \
  && refused --kafka iggy --confluent-bootstrap "$bs" --confluent-topics t \
  && refused --confluent-topics t \
  && ok "confluent needs a bootstrap and an instance; --confluent-* needs --kafka confluent" || bad "missing input accepted"
refused --kafka confluent --confluent-bootstrap 'h:1;id' --confluent-topics t \
  && refused --kafka confluent --confluent-bootstrap "$bs" --confluent-topics 't|x' \
  && refused --kafka confluent --confluent-bootstrap "$bs" --confluent-sinks onlystream \
  && refused --kafka confluent --confluent-bootstrap "$bs" --confluent-topics t --confluent-integration Bad \
  && refused --kafka confluent --confluent-bootstrap "$bs" --confluent-topics t --confluent-rate 100 \
  && refused --kafka confluent --confluent-bootstrap "$bs" --confluent-topics a.b,a_b \
  && ok "unsafe or colliding input is refused" || bad "unsafe input accepted"

exit "$fail"
