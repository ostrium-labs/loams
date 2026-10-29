// Single-statement latency from the Worker (nearest colo to this machine) and from a Durable
// Object created with locationHint=enam (near aws-us-east-1). Writes results/bench.json.
import { writeFileSync } from "node:fs";
import { call, pct } from "./lib.ts";

const out: any = {};
for (const where of ["worker", "do-enam"]) {
  for (const kind of ["noop", "insert", "upsert", "update", "select"]) {
    const samples: number[] = [];
    let colo = null, errors = 0;
    for (let i = 0; i < 3; i++) {
      const path = where === "worker" ? `/bench?kind=${kind}&n=40` : `/u/hib/bench-enam/bench?kind=${kind}&n=40&hint=enam`;
      const r = await call("GET", path, undefined, { timeoutMs: 120_000 });
      samples.push(...r.data.samples);
      errors += r.data.errors.length;
      colo = r.data.colo;
    }
    const first = samples[0];
    out[`${where}/${kind}`] = { colo, n: samples.length, errors, p50: pct(samples, 50), p95: pct(samples, 95), max: Math.max(...samples), min: Math.min(...samples), first };
    console.log(where, kind, JSON.stringify(out[`${where}/${kind}`]));
  }
}
writeFileSync("results/bench.json", JSON.stringify(out, null, 2));
