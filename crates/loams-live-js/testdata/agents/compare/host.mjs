// The Node and Bun comparison of the agent-profile spike (MS1 Task 1):
// runs a fixture bundle with the same scripted hosts as the QuickJS
// profile's `agent::fake` (in process here, frames there) and the same
// ACP stdio surface. Measurements only:
//
//   node host.mjs <bundle.js>      bun host.mjs <bundle.js>
//
// The bundle's `import … from "loams:agent"` is pointed at shim.mjs
// through a rewritten copy in a temporary directory.

import { createInterface } from "node:readline";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { deliver } from "./shim.mjs";

const shim = pathToFileURL(join(dirname(fileURLToPath(import.meta.url)), "shim.mjs")).href;
const source = readFileSync(process.argv[2], "utf8").replaceAll('"loams:agent"', JSON.stringify(shim));
const copy = join(mkdtempSync(join(tmpdir(), "agent-cmp-")), "agent.mjs");
writeFileSync(copy, source);
await import(pathToFileURL(copy).href);

for await (const line of createInterface({ input: process.stdin })) {
  if (line.trim()) deliver(JSON.parse(line));
}
