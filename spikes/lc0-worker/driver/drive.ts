// The 20-device load for one variant. Usage:
//   node driver/drive.ts --variant hib|auto|nohib --user <id> [--devices 20] [--minutes 60] [--align] --out results/<name>.json
// Each device keeps one WebSocket to the user's Durable Object and pings every 30 s (random phase,
// or aligned to wall-clock :00/:30 with --align). Device d00 holds a run lease for the whole run.
import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { call, counters, Device, sleep } from "./lib.ts";

const { values: a } = parseArgs({
  options: {
    variant: { type: "string" }, user: { type: "string" }, devices: { type: "string", default: "20" },
    minutes: { type: "string", default: "60" }, align: { type: "boolean", default: false }, out: { type: "string" },
  },
});
const N = Number(a.devices), MIN = Number(a.minutes), PING = 30_000;
const variant = a.variant!, user = a.user!;
const t0 = Date.now();
const devs = Array.from({ length: N }, (_, i) => new Device(variant, user, `d${String(i).padStart(2, "0")}`));
let reconnects = 0, connectErrors = 0, seq = 0, stop = false;
const states: any[] = [];

async function keep(d: Device) {
  while (!stop) {
    try {
      await d.connect();
      await new Promise<void>((res) => {
        d.ws!.onclose = () => res();
        d.onDead = () => res();
      });
      if (stop) return;
      reconnects++;
    } catch {
      connectErrors++;
    }
    await sleep(1000 + Math.random() * 2000);
  }
}

for (const d of devs) void keep(d);
await sleep(3000);
const claim = await devs[0].claim("run-1").catch((e) => ({ error: String(e) }));

const timers = devs.map((d, i) => {
  const phase = a.align ? PING - (Date.now() % PING) + Math.random() * 300 : Math.random() * PING;
  let iv: ReturnType<typeof setInterval>;
  const to = setTimeout(() => {
    d.ping(seq++);
    iv = setInterval(() => d.ping(seq++), PING);
  }, phase);
  return () => (clearTimeout(to), clearInterval(iv));
});

const end = t0 + MIN * 60_000;
while (Date.now() < end) {
  await sleep(Math.min(600_000, end - Date.now()));
  const s = await call("GET", `/u/${variant}/${user}/state`).catch((e) => ({ status: 0, data: String(e) }));
  states.push({ at_min: (Date.now() - t0) / 60000, ...(s.data as any), presence: undefined, online: (s.data as any)?.presence?.filter((p: any) => p.state === "online").length });
  console.log(new Date().toISOString(), variant, JSON.stringify(states.at(-1)));
}
stop = true;
timers.forEach((f) => f());
const final = await call("GET", `/u/${variant}/${user}/state`);
devs.forEach((d) => d.close());
const hours = (Date.now() - t0) / 3600_000;
const wakes = new Set(devs.flatMap((d) => [...d.wakes]));
const pings = counters.ws_sent;
const out = {
  variant, user, devices: N, minutes: (Date.now() - t0) / 60000, align: a.align, claim,
  counters, reconnects, connectErrors, dead_sockets_detected: devs.reduce((s, d) => s + d.deadSockets, 0), distinct_wakes_seen_in_pongs: wakes.size,
  pongs: devs.reduce((s, d) => s + d.pongs, 0),
  do_wakes_total: final.data.wakes, final_leases: final.data.leases,
  final_online: final.data.presence?.filter((p: any) => p.state === "online").length,
  per_device_per_day: {
    ws_messages_in: (pings / N / hours) * 24,
    worker_requests: ((counters.ws_upgrades + counters.http) / N / hours) * 24,
  },
  states,
};
writeFileSync(a.out!, JSON.stringify(out, null, 2));
console.log(JSON.stringify({ ...out, states: undefined }));
process.exit(0);
