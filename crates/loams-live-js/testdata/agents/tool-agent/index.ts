// The spike's tool agent: per turn one streamed model call (agentd's model
// proxy), one MCP tool call (agentd's MCP shim), one fetch (agentd's
// egress proxy) and a timer. The prompt text picks a scenario for the
// tests: "weather" (the default), "fetch <url>", "list", "call <tool>",
// "timers", "token", "hang" (waits until cancelled), "random", and for
// the measurements "bench" (host-call round trips, timed inside the
// agent).
import { acp, loams, mcp, model } from "loams:agent";

type Msg = { jsonrpc?: string; id?: number | string; method?: string; params?: any };

const turns = new Map<string, AbortController>();
let sessions = 0;

const conn = acp.connect(async (m: Msg) => {
  const reply = (result: unknown) => conn.send({ jsonrpc: "2.0", id: m.id, result });
  switch (m.method) {
    case "initialize":
      return reply({
        protocolVersion: 1,
        agentCapabilities: { loadSession: false },
        authMethods: [],
        agentInfo: { name: "tool-agent", version: "0.1.0" },
      });
    case "session/new":
      return reply({ sessionId: `tool-${++sessions}` });
    case "session/cancel":
      turns.get(m.params.sessionId)?.abort();
      return;
    case "session/prompt": {
      const sessionId: string = m.params.sessionId;
      const controller = new AbortController();
      turns.set(sessionId, controller);
      try {
        await turn(sessionId, text(m.params.prompt), controller.signal);
        reply({ stopReason: controller.signal.aborted ? "cancelled" : "end_turn" });
      } catch (e) {
        if (controller.signal.aborted) reply({ stopReason: "cancelled" });
        else conn.send({ jsonrpc: "2.0", id: m.id, error: { code: -32603, message: String(e) } });
      } finally {
        turns.delete(sessionId);
      }
      return;
    }
    default:
      if (m.id !== undefined) {
        conn.send({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: `method not found: ${m.method}` } });
      }
  }
});

function text(prompt: { type: string; text?: string }[]): string {
  return prompt.filter((b) => b.type === "text").map((b) => b.text ?? "").join("");
}

function update(sessionId: string, u: Record<string, unknown>) {
  conn.send({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update: u } });
}

function say(sessionId: string, t: string) {
  update(sessionId, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: t } });
}

const aborted = (signal: AbortSignal) =>
  new Promise<never>((_, reject) => signal.addEventListener("abort", () => reject(signal.reason)));

async function turn(sessionId: string, prompt: string, signal: AbortSignal) {
  const [command, ...rest] = prompt.trim().split(/\s+/);
  switch (command) {
    case "fetch": {
      try {
        const res = await fetch(rest[0], { method: "POST", body: JSON.stringify({ hello: "world" }), signal });
        const body = await res.arrayBuffer();
        say(sessionId, `fetched ${res.status} ${body.byteLength} bytes`);
      } catch (e) {
        say(sessionId, `fetch failed: ${e}`);
      }
      return;
    }
    case "list": {
      const { tools } = await mcp.list("weather");
      say(sessionId, tools.map((t) => t.name).join(","));
      return;
    }
    case "call": {
      try {
        await mcp.call("weather", rest[0], {});
        say(sessionId, "called");
      } catch (e) {
        say(sessionId, `call failed: ${e}`);
      }
      return;
    }
    case "timers": {
      const order: string[] = [];
      const cleared = setTimeout(() => order.push("cleared"), 5);
      clearTimeout(cleared);
      let ticks = 0;
      const interval = setInterval(() => {
        order.push(`tick${++ticks}`);
        if (ticks === 2) clearInterval(interval);
      }, 10);
      await new Promise<void>((r) => setTimeout(() => { order.push("t40"); r(); }, 40));
      setTimeout(() => order.push("t5"), 5);
      queueMicrotask(() => order.push("micro"));
      await new Promise<void>((r) => setTimeout(r, 20));
      say(sessionId, order.join(","));
      return;
    }
    case "token":
      say(sessionId, await loams.token());
      return;
    case "random": {
      const bytes = crypto.getRandomValues(new Uint8Array(8));
      say(sessionId, JSON.stringify({ uuid: crypto.randomUUID(), bytes: bytes.length, r: Math.random() < 1 }));
      return;
    }
    case "bench":
      say(sessionId, JSON.stringify(await bench()));
      return;
    case "hang":
      say(sessionId, "waiting");
      await Promise.race([model.chat({ hang: true, messages: [] }), aborted(signal)]);
      return;
    default:
      return weather(sessionId, prompt, signal);
  }
}

// Mean milliseconds per call: an empty host call (the token), a 1 MiB
// fetch body, and a streamed model response (9 events).
async function bench() {
  const time = async (n: number, f: () => Promise<unknown>) => {
    const start = performance.now();
    for (let i = 0; i < n; i++) await f();
    return Number(((performance.now() - start) / n).toFixed(4));
  };
  return {
    emptyMs: await time(500, () => loams.token()),
    fetch1MiBMs: await time(20, async () => (await fetch("https://api.example.com/bytes/1048576")).arrayBuffer()),
    streamMs: await time(50, async () => {
      const s = model.chat({ messages: [{ role: "user", content: "hi" }] }, { stream: true });
      for await (const _ of s) {
      }
      return s.result;
    }),
  };
}

async function weather(sessionId: string, prompt: string, signal: AbortSignal) {
  const messages: any[] = [{ role: "user", content: prompt }];
  const tools = [{ type: "function", function: { name: "weather__get_forecast", parameters: { type: "object" } } }];

  // 1. The model, streamed.
  const stream = model.chat({ model: "fake-model", messages, tools }, { stream: true });
  for await (const event of stream) {
    const delta = event.choices?.[0]?.delta?.content;
    if (delta) say(sessionId, delta);
  }
  const response = await stream.result;
  const call = response.choices[0].message.tool_calls?.[0];

  // A timer between the steps.
  await new Promise<void>((r) => setTimeout(r, 1));
  signal.throwIfAborted();

  // 2. The tool, through the MCP shim.
  if (call) {
    const [server, tool] = call.function.name.split("__");
    const args = JSON.parse(call.function.arguments);
    update(sessionId, { sessionUpdate: "tool_call", toolCallId: call.id, title: `${server}/${tool}`, kind: "fetch", status: "in_progress", rawInput: args });
    const result = await mcp.call(server, tool, args);
    const out = result.content.map((c: any) => c.text ?? "").join("");
    update(sessionId, {
      sessionUpdate: "tool_call_update",
      toolCallId: call.id,
      status: "completed",
      content: [{ type: "content", content: { type: "text", text: out } }],
    });
    say(sessionId, `\n${out}`);
  }

  // 3. A fetch, through the egress proxy.
  const res = await fetch("https://api.example.com/v1/notes", { method: "POST", body: JSON.stringify({ prompt }), signal });
  say(sessionId, `\nnoted (${res.status})`);
}

export default { name: "tool-agent", version: "0.1.0" };
