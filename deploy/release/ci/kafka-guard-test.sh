#!/usr/bin/env bash
# Self-test of config/kafka-guard.sh (D1798): the `kafka` profile refuses to
# start while Iggy's root password is empty, a .env.example placeholder or too
# short, says so without printing it, and otherwise runs the service's command.
# Also checks that both services of the profile start through the guard, with
# .env.example's own value refused. Needs sh, jq and `docker compose`.
#
#   deploy/release/ci/kafka-guard-test.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
guard=config/kafka-guard.sh
fail=0

# expect <want exit> <password or "-unset-"> <stderr must contain>
expect() {
  local want="$1" password="$2" needle="$3" out status
  if [[ "$password" == "-unset-" ]]; then
    out="$(env -u KAFKA_TEST_PW sh "$guard" KAFKA_TEST_PW sh -c 'echo ran' 2>&1)" && status=0 || status=$?
  else
    out="$(KAFKA_TEST_PW="$password" sh "$guard" KAFKA_TEST_PW sh -c 'echo ran' 2>&1)" && status=0 || status=$?
  fi
  if [[ "$status" != "$want" ]]; then
    echo "FAIL: password '${password}': exit $status, wanted $want: $out"; fail=1; return
  fi
  if [[ "$out" != *"$needle"* ]]; then
    echo "FAIL: password '${password}': output lacks '$needle': $out"; fail=1; return
  fi
  if [[ "$password" != "-unset-" && ${#password} -ge 4 && "$want" != 0 && "$out" == *"$password"* ]]; then
    echo "FAIL: the refusal printed the password"; fail=1
  fi
}

example="$(sed -n 's/^KAFKA_IGGY_PASSWORD=//p' .env.example)"
[[ -n "$example" ]] || { echo "FAIL: .env.example has no KAFKA_IGGY_PASSWORD"; exit 1; }

expect 78 -unset- "is empty; refusing to start"
expect 78 "" "is empty; refusing to start"
expect 78 "$example" "still the .env.example placeholder"
expect 78 change-me "still the .env.example placeholder"
expect 78 loams-kafka-preview "still the .env.example placeholder"
expect 78 short-pw1 "shorter than 12 characters"
expect 0 "a-password-of-my-own-1" "ran"
# A bad variable name or no command is a usage error, not a start.
out="$(sh "$guard" 'BAD NAME' true 2>&1)" && status=0 || status=$?
[[ "$status" == 64 ]] || { echo "FAIL: bad variable name: exit $status: $out"; fail=1; }
out="$(sh "$guard" KAFKA_TEST_PW 2>&1)" && status=0 || status=$?
[[ "$status" == 64 ]] || { echo "FAIL: no command: exit $status: $out"; fail=1; }

# The compose wiring: both services go through the guard with the variable
# that carries KAFKA_IGGY_PASSWORD, and .env.example's value reaches them (so
# an unedited .env is refused).
cfg="$(COMPOSE_PROFILES=kafka LOAMS_POSTGRES_COMPOSE=./postgres-off.yaml \
  docker compose --env-file .env.example config --format json)"
for pair in iggy:IGGY_ROOT_PASSWORD kafka-gateway:IGGY_KAFKA_IGGY_PASSWORD; do
  s="${pair%%:*}" v="${pair#*:}"
  jq -e --arg s "$s" --arg v "$v" --arg pw "$example" '.services[$s]
     | (.entrypoint == ["/bin/sh", "/etc/loams/kafka-guard.sh", $v])
       and (.environment[$v] == $pw)
       and ([.volumes[] | select(.target == "/etc/loams/kafka-guard.sh" and .read_only == true)] | length == 1)' \
    <<<"$cfg" >/dev/null || { echo "FAIL: $s does not start through the guard with $v"; fail=1; }
done

[[ $fail == 0 ]] || exit 1
echo "kafka guard self-test ok"
