// An agent on the ACP TypeScript SDK, unmodified apart from the bundling:
// the Web-streams polyfill is the bundle's, `acp.stream()` is the host's.
import "web-streams-polyfill/polyfill";
import * as sdk from "@agentclientprotocol/sdk";
import { acp } from "loams:agent";

class SdkAgent implements sdk.Agent {
  constructor(private readonly conn: sdk.AgentSideConnection) {}

  async initialize(_params: sdk.InitializeRequest): Promise<sdk.InitializeResponse> {
    return {
      protocolVersion: sdk.PROTOCOL_VERSION,
      agentCapabilities: { loadSession: false },
      authMethods: [],
    };
  }

  async newSession(_params: sdk.NewSessionRequest): Promise<sdk.NewSessionResponse> {
    return { sessionId: crypto.randomUUID() };
  }

  async authenticate(_params: sdk.AuthenticateRequest): Promise<sdk.AuthenticateResponse> {
    return {};
  }

  async prompt(params: sdk.PromptRequest): Promise<sdk.PromptResponse> {
    const text = params.prompt.map((b) => (b.type === "text" ? b.text : "")).join("");
    await this.conn.sessionUpdate({
      sessionId: params.sessionId,
      update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: `sdk: ${text}` } },
    });
    return { stopReason: "end_turn" };
  }

  async cancel(_params: sdk.CancelNotification): Promise<void> {}
}

const { input, output } = acp.stream();
new sdk.AgentSideConnection((conn) => new SdkAgent(conn), sdk.ndJsonStream(output, input));

export default { name: "acp-sdk", version: "0.1.0" };
