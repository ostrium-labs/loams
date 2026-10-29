#!/usr/bin/env bash
# Restart only the connect role against the running engine:
#   connect.sh restart   SIGTERM then start
#   connect.sh kill9     SIGKILL then start (what e2e.sh does mid-run)
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
case ${1:-restart} in
  kill9)   pid_alive "$CFB_RUN/connect.pid" && kill -KILL "$(cat "$CFB_RUN/connect.pid")" ;;
  restart) pid_alive "$CFB_RUN/connect.pid" && kill -TERM "$(cat "$CFB_RUN/connect.pid")" ;;
  *) die "usage: connect.sh restart|kill9" ;;
esac
for _ in $(seq 1 40); do pid_alive "$CFB_RUN/connect.pid" || break; sleep 0.25; done
start_connect
wait_instances_running
