// Picks a working `python3`, so the workspace check runs on Windows too.
import { spawnSync } from "node:child_process";

const candidates = [
  { command: "python3", args: [] },
  { command: "python", args: [] },
  { command: "py", args: ["-3"] },
];

const script = process.argv.slice(2);
for (const { command, args } of candidates) {
  const probe = spawnSync(command, [...args, "--version"], { stdio: "ignore" });
  if (probe.status === 0) {
    const run = spawnSync(command, [...args, ...script], { stdio: "inherit" });
    process.exit(run.status ?? 1);
  }
}

console.error(
  `No Python interpreter found. Tried: ${candidates.map((c) => c.command).join(", ")}`,
);
process.exit(127);