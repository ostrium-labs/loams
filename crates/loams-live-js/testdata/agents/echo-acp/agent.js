// echo-acp/index.js
import { acp } from "loams:agent";
var sessions = 0;
var conn = acp.connect((m) => {
  const reply = (result) => conn.send({ jsonrpc: "2.0", id: m.id, result });
  switch (m.method) {
    case "initialize":
      return reply({
        protocolVersion: 1,
        agentCapabilities: { loadSession: false },
        authMethods: [],
        agentInfo: { name: "echo-acp", version: "0.1.0" }
      });
    case "session/new":
      return reply({ sessionId: `echo-${++sessions}` });
    case "session/prompt": {
      const text = m.params.prompt.filter((b) => b.type === "text").map((b) => b.text).join("");
      conn.send({
        jsonrpc: "2.0",
        method: "session/update",
        params: {
          sessionId: m.params.sessionId,
          update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: `echo: ${text}` } }
        }
      });
      return reply({ stopReason: "end_turn" });
    }
    case "session/cancel":
      return;
    default:
      if (m.id !== void 0) {
        conn.send({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: `method not found: ${m.method}` } });
      }
  }
});
var index_default = { name: "echo-acp", version: "0.1.0" };
export {
  index_default as default
};
