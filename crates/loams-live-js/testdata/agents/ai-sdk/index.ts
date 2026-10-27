// The AI SDK's `generateText` through an OpenAI-compatible provider whose
// `fetch` is the host's (agentd's egress proxy, which routes the model
// proxy's URL). Expected to fail in places; its failure is a finding.
// Web streams: QuickJS has none, and the AI SDK builds TransformStreams at
// module load ("TransformStream is not defined" without this polyfill).
import "web-streams-polyfill/polyfill";
import { createOpenAICompatible } from "@ai-sdk/openai-compatible";
import { generateText } from "ai";
import { acp } from "loams:agent";

const provider = createOpenAICompatible({
  name: "loams",
  baseURL: "https://model-proxy.loams.local/v1",
  apiKey: "dummy",
  fetch: (input, init) => fetch(input, init),
});

let sessions = 0;
const conn = acp.connect(async (m: any) => {
  const reply = (result: unknown) => conn.send({ jsonrpc: "2.0", id: m.id, result });
  switch (m.method) {
    case "initialize":
      return reply({ protocolVersion: 1, agentCapabilities: { loadSession: false }, authMethods: [] });
    case "session/new":
      return reply({ sessionId: `ai-${++sessions}` });
    case "session/prompt": {
      try {
        const prompt = m.params.prompt.map((b: any) => b.text ?? "").join("");
        const { text } = await generateText({ model: provider("fake-model"), prompt });
        conn.send({
          jsonrpc: "2.0",
          method: "session/update",
          params: { sessionId: m.params.sessionId, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } },
        });
        return reply({ stopReason: "end_turn" });
      } catch (e: any) {
        return conn.send({ jsonrpc: "2.0", id: m.id, error: { code: -32603, message: `${e}\n${e?.stack ?? ""}` } });
      }
    }
    default:
      if (m.id !== undefined) conn.send({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "method not found" } });
  }
});

export default { name: "ai-sdk", version: "0.1.0" };
