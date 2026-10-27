#!/bin/sh
# Fake `goose acp` for tests/goose.rs (MS1 Task 3): answers the ACP
# handshake the way goose 1.54.0 does and records what the harness handed it
# (argv, the environment, the hints file, `session/new`) under
# `$GOOSE_PATH_ROOT/../record`, so the test can check the launch contract
# without a real goose or a network.

emit() { printf '%s\n' "$1"; }
rid() { printf '%s' "$1" | sed 's/.*"id":\([0-9]*\).*/\1/'; }
has() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }
update() {
  emit "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"$SID\",\"update\":$1}}"
}

REC="$GOOSE_PATH_ROOT/../record"
mkdir -p "$REC" || exit 1
printf '%s\n' "$*" >"$REC/args"
env >"$REC/env"
if [ -f "$GOOSE_PATH_ROOT/config/.goosehints" ]; then
  cat "$GOOSE_PATH_ROOT/config/.goosehints" >"$REC/hints"
fi

read -r line || exit 1
has "$line" '"method":"initialize"' || exit 1
emit "{\"jsonrpc\":\"2.0\",\"id\":$(rid "$line"),\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"loadSession\":true,\"promptCapabilities\":{\"image\":true,\"audio\":false,\"embeddedContext\":true},\"mcpCapabilities\":{\"http\":true,\"sse\":false}},\"authMethods\":[{\"id\":\"goose-provider\",\"name\":\"Configure Provider\"}],\"agentInfo\":{\"name\":\"goose\",\"version\":\"1.54.0\"}}}"

read -r line || exit 1
has "$line" '"method":"session/new"' || exit 1
printf '%s\n' "$line" >"$REC/session_new"
SID="20261025_1"
emit "{\"jsonrpc\":\"2.0\",\"id\":$(rid "$line"),\"result\":{\"sessionId\":\"$SID\",\"modes\":{\"currentModeId\":\"approve\",\"availableModes\":[{\"id\":\"auto\",\"name\":\"auto\"},{\"id\":\"approve\",\"name\":\"approve\"}]},\"configOptions\":[{\"id\":\"mode\",\"name\":\"Mode\",\"category\":\"mode\",\"type\":\"select\",\"currentValue\":\"approve\",\"options\":[{\"value\":\"auto\",\"name\":\"auto\"},{\"value\":\"approve\",\"name\":\"approve\"}]},{\"id\":\"model\",\"name\":\"Model\",\"category\":\"model\",\"type\":\"select\",\"currentValue\":\"$GOOSE_MODEL\",\"options\":[{\"value\":\"gpt-5.4\",\"name\":\"GPT-5.4\"},{\"value\":\"gpt-5.5\",\"name\":\"GPT-5.5\"}]}]}}"

read -r line || exit 1
while has "$line" '"method":"session/set_config_option"' || has "$line" '"method":"session/set_model"'; do
  printf '%s\n' "$line" >>"$REC/config_sets"
  emit "{\"jsonrpc\":\"2.0\",\"id\":$(rid "$line"),\"result\":{}}"
  read -r line || exit 1
done
has "$line" '"method":"session/prompt"' || exit 1
pid=$(rid "$line")

case "$line" in
*scenario:permission*)
  # Verbatim shape of goose 1.54.0 in `approve` mode (captured live): the
  # tool call is announced, then permission is asked under a string id with
  # the catch-all kind `other` and a `<tool> · <summary>` title.
  update '{"sessionUpdate":"tool_call","toolCallId":"call_x","title":"shell · rm -rf build","rawInput":{"command":"rm -rf build"},"_meta":{"goose":{"toolCall":{"toolName":"shell","extensionName":"developer"}}}}'
  emit "{\"jsonrpc\":\"2.0\",\"id\":\"625d36a4-b0d8-4a1d-a413-3612b4602054\",\"method\":\"session/request_permission\",\"params\":{\"sessionId\":\"$SID\",\"toolCall\":{\"toolCallId\":\"call_x\",\"kind\":\"other\",\"status\":\"pending\",\"title\":\"shell · rm -rf build\",\"rawInput\":{\"command\":\"rm -rf build\"}},\"options\":[{\"optionId\":\"allow_always\",\"name\":\"allow_always\",\"kind\":\"allow_always\"},{\"optionId\":\"allow_once\",\"name\":\"allow_once\",\"kind\":\"allow_once\"},{\"optionId\":\"reject_once\",\"name\":\"reject_once\",\"kind\":\"reject_once\"},{\"optionId\":\"reject_always\",\"name\":\"reject_always\",\"kind\":\"reject_always\"}]}}"
  read -r answer || exit 1
  printf '%s\n' "$answer" >"$REC/permission_answer"
  if has "$answer" '"optionId":"allow_once"'; then
    update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"tool allowed"}}'
  else
    update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"tool rejected"}}'
  fi
  ;;
*)
  update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello from goose"}}'
  ;;
esac
emit "{\"jsonrpc\":\"2.0\",\"id\":$pid,\"result\":{\"stopReason\":\"end_turn\",\"usage\":{\"totalTokens\":9,\"inputTokens\":5,\"outputTokens\":4}}}"
# goose keeps serving until stdin closes.
while read -r line; do :; done
