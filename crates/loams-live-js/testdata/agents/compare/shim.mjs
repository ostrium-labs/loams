// The `loams:agent` shim of the Node/Bun comparison (see host.mjs): the
// same scripted hosts as the QuickJS profile's `agent::fake`, in process.

const MODEL_TEXT = "Let me check the forecast for Paris.";

function completion(request) {
  const wantsTools = Array.isArray(request?.tools) && request.tools.length > 0;
  const hasToolResult = (request?.messages ?? []).some((m) => m.role === "tool");
  const message =
    hasToolResult || !wantsTools
      ? { role: "assistant", content: "It is sunny in Paris." }
      : {
          role: "assistant",
          content: MODEL_TEXT,
          tool_calls: [{ id: "call-1", type: "function", function: { name: "weather__get_forecast", arguments: '{"city":"Paris"}' } }],
        };
  return { id: "chatcmpl-fake", object: "chat.completion", created: 0, model: "fake-model", choices: [{ index: 0, message, finish_reason: "stop" }] };
}

globalThis.fetch = async (input, init = {}) => {
  const url = String(input instanceof Request ? input.url : input);
  const bytes = /\/bytes\/(\d+)$/.exec(url);
  if (url.endsWith("/chat/completions")) {
    return new Response(JSON.stringify(completion(JSON.parse(init.body ?? "null"))), { headers: { "content-type": "application/json" } });
  }
  if (bytes) return new Response(new Uint8Array(Number(bytes[1])).fill(120));
  return new Response(JSON.stringify({ url, method: init.method ?? "GET" }), { headers: { "content-type": "application/json" } });
};

let handler = null;
const backlog = [];
const write = (m) => process.stdout.write((typeof m === "string" ? m : JSON.stringify(m)) + "\n");

export const acp = {
  connect(h) {
    handler = h;
    for (const m of backlog.splice(0)) Promise.resolve().then(() => handler(m));
    return { send: write };
  },
  stream() {
    let controller;
    const input = new ReadableStream({ start(c) { controller = c; } });
    const enc = new TextEncoder();
    acp.connect((m) => controller.enqueue(enc.encode(JSON.stringify(m) + "\n")));
    const dec = new TextDecoder();
    const output = new WritableStream({ write(chunk) { process.stdout.write(dec.decode(chunk)); } });
    return { input, output };
  },
};

function modelCall(request, options = {}) {
  if (request?.hang) return new Promise(() => {});
  const response = completion(request);
  if (!options.stream) return Promise.resolve(response);
  const text = response.choices[0].message.content;
  return {
    result: Promise.resolve(response),
    async *[Symbol.asyncIterator]() {
      for (const w of text.split(/(?<= )/)) yield { choices: [{ index: 0, delta: { content: w } }] };
    },
  };
}
export const model = { chat: modelCall, responses: modelCall, messages: modelCall };
export const mcp = {
  list: async () => ({ tools: [{ name: "get_forecast" }] }),
  call: async (server, tool, args) => ({ content: [{ type: "text", text: `${server}/${tool}: Sunny, 21°C in ${args?.city ?? "?"}` }], isError: false }),
};
export const loams = { token: async () => "place-token-fake", config: () => ({}) };


export function deliver(m) {
  if (handler) Promise.resolve().then(() => handler(m));
  else backlog.push(m);
}
