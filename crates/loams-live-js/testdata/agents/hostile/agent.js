// hostile/index.js
import { acp } from "loams:agent";
function attempt(name, f) {
  try {
    const v = f();
    return `${name}=${typeof v === "object" ? JSON.stringify(v) : String(v)}`;
  } catch (e) {
    return `${name}!${e && e.name}`;
  }
}
async function probe() {
  const results = [
    attempt("eval", () => (0, eval)("1 + 1")),
    attempt("Function", () => new Function("return 1")()),
    attempt("ctorChain", () => (() => {
    }).constructor("return 1")()),
    attempt("asyncCtor", () => Object.getPrototypeOf(async () => {
    }).constructor("return 1")),
    attempt("require", () => typeof globalThis.require),
    attempt("process", () => typeof process),
    attempt("WebAssembly", () => typeof WebAssembly),
    attempt("std", () => typeof globalThis.std + "/" + typeof globalThis.os)
  ];
  for (const mod of ["node:fs", "fs", "qjs:os", "os", "std", "loams:server"]) {
    try {
      await import(mod);
      results.push(`import(${mod})=ok`);
    } catch (e) {
      results.push(`import(${mod})!${e && e.name}`);
    }
  }
  return results.join(";");
}
var conn = acp.connect(async (m) => {
  const reply = (result) => conn.send({ jsonrpc: "2.0", id: m.id, result });
  if (m.method === "initialize") return reply({ protocolVersion: 1, agentCapabilities: {}, authMethods: [] });
  if (m.method === "session/new") return reply({ sessionId: "hostile" });
  if (m.method !== "session/prompt") return;
  const command = m.params.prompt.map((b) => b.text).join("").trim();
  let text = "";
  if (command === "probe") text = await probe();
  else if (command === "spin") for (; ; ) {
  }
  else if (command === "builtin") Array.prototype.reverse.call({ length: 2 ** 53 - 1 });
  else if (command === "memory") {
    const hoard = [];
    for (; ; ) hoard.push(new Array(1e5).fill(command));
  }
  conn.send({
    jsonrpc: "2.0",
    method: "session/update",
    params: { sessionId: "hostile", update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } }
  });
  reply({ stopReason: "end_turn" });
});
var index_default = { name: "hostile", version: "0.1.0" };
export {
  index_default as default
};
