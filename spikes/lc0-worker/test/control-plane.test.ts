// LC0 Task 3 tests, run against the deployed spike Worker (never in CI: they need the owner's accounts).
//   LC0_URL=... LC0_BEARER_FILE=... node --test test/
// teardown_leaves_nothing runs only with LC0_TEARDOWN=1, after teardown.sh, and needs the
// Cloudflare and TiDB env files sourced into its environment.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { homedir } from "node:os";

const TEARDOWN = process.env.LC0_TEARDOWN === "1";
const lib = TEARDOWN ? null : await import("../driver/lib.ts");
const rnd = () => Math.random().toString(36).slice(2, 8);

test("lease_is_exclusive_under_races", { skip: TEARDOWN, timeout: 300_000 }, async (t) => {
  const { call, Device } = lib!;

  await t.test("20 devices race through the user's Durable Object", async () => {
    const user = `t-race-${rnd()}`;
    const devs = Array.from({ length: 20 }, (_, i) => new Device("hib", user, `d${i}`));
    await Promise.all(devs.map((d) => d.connectWithRetry()));
    for (let round = 0; round < 10; round++) {
      const run = `r${round}`;
      const rs = await Promise.all(devs.map((d) => d.claim(run)));
      const winners = rs.filter((r) => r.ok);
      assert.equal(winners.length, 1, `round ${round}: exactly one winner`);
      const w = winners[0];
      for (const r of rs.filter((r) => !r.ok)) assert.equal(r.holder, w.holder);
      const db = await call("GET", `/db-lease/get?user=${user}&run=${run}`);
      assert.equal(db.data.lease.holder, w.holder);
      assert.equal(Number(db.data.lease.epoch), 1);
      assert.equal(db.data.events.length, 1);
    }
    devs.forEach((d) => d.close());
  });

  await t.test("20 concurrent HTTP claims on TiDB's conditional single statements", async () => {
    const user = `t-dbrace-${rnd()}`;
    for (let round = 0; round < 10; round++) {
      const run = `r${round}`;
      const rs = await Promise.all(
        Array.from({ length: 20 }, (_, i) => call("POST", "/db-lease/claim", { user, run, device: `d${i}`, ttl: 60_000 })),
      );
      assert.ok(rs.every((r) => r.status === 200), JSON.stringify(rs.filter((r) => r.status !== 200).map((r) => r.data)));
      const winners = rs.filter((r) => r.data.won);
      assert.equal(winners.length, 1, `round ${round}: exactly one winner`);
      const db = await call("GET", `/db-lease/get?user=${user}&run=${run}`);
      assert.equal(db.data.lease.holder, winners[0].data.holder);
    }
  });

  await t.test("after expiry, exactly one of 20 racers takes the lease and the epoch moves on", async () => {
    const user = `t-expiry-${rnd()}`;
    const first = await call("POST", "/db-lease/claim", { user, run: "r", device: "old", ttl: 2000 });
    assert.equal(first.data.won, true);
    await new Promise((r) => setTimeout(r, 3500));
    const rs = await Promise.all(Array.from({ length: 20 }, (_, i) => call("POST", "/db-lease/claim", { user, run: "r", device: `d${i}`, ttl: 60_000 })));
    const winners = rs.filter((r) => r.data.won);
    assert.equal(winners.length, 1);
    assert.equal(winners[0].data.epoch, 2);
    const stale = await call("POST", "/db-lease/claim", { user, run: "r", device: "old", ttl: 60_000 });
    assert.equal(stale.data.won, false, "the old holder cannot take back a live lease");
  });
});

test("device_reconnects_after_sleep", { skip: TEARDOWN, timeout: 300_000 }, async () => {
  const { call, Device, sleep } = lib!;
  const user = `t-sleep-${rnd()}`;
  const [d0, d1, d2, d3, d4] = ["d0", "d1", "d2", "d3", "d4"].map((id) => new Device("hib", user, id));
  const all = [d0, d1, d2, d3, d4];
  await Promise.all(all.map((d) => d.connectWithRetry()));
  let seq = 0;
  const pinger = setInterval(() => all.forEach((d) => d.ping(seq++)), 30_000);
  const state = async () => (await call("GET", `/u/hib/${user}/state`)).data;
  const presence = (s: any, id: string) => s.presence.find((p: any) => p.device === id)?.state;
  const wake = async (d: InstanceType<typeof Device>) => {
    d.close(); // after sleep the old socket is presumed dead: reconnect
    d.asleep = false;
    await d.connectWithRetry();
    d.ping(seq++);
  };

  assert.equal((await d0.claim("keep")).ok, true);
  assert.equal((await d2.claim("steal")).ok, true);
  assert.equal((await d4.claim("resume")).ok, true);
  const e0 = (await call("GET", `/db-lease/get?user=${user}&run=resume`)).data.lease.epoch;

  // Short sleep (40 s, inside the 60 s lease): nobody can take the run.
  d0.asleep = d2.asleep = d4.asleep = true;
  await sleep(36_000);
  assert.equal((await d1.claim("keep")).ok, false, "a 36 s-old lease is still held");
  await wake(d0);
  const k = await d0.claim("keep");
  assert.equal(k.ok, true, "the sleeper renews its lease after a short sleep");
  assert.equal(k.epoch, 1);

  // Long sleep (80 s, past two missed pings and the lease).
  await sleep(36_000); // d2 and d4 have been asleep ~72 s
  const s = await state();
  assert.equal(presence(s, "d2"), "asleep");
  assert.equal(presence(s, "d4"), "asleep");
  assert.equal(presence(s, "d1"), "online");
  const steal = await d3.claim("steal");
  assert.equal(steal.ok, true, "another device may take an expired lease");
  assert.equal(steal.epoch, 2);
  await sleep(8_000);
  await wake(d2);
  const back = await d2.claim("steal");
  assert.equal(back.ok, false, "the sleeper finds its run taken and must stop");
  assert.equal(back.holder, "d3");
  await wake(d4);
  const r = await d4.claim("resume");
  assert.equal(r.ok, true, "with nobody else holding it, the sleeper takes its run back");
  assert.equal(r.epoch, 1, "same holder: no new epoch");
  assert.equal((await call("GET", `/db-lease/get?user=${user}&run=resume`)).data.lease.epoch, e0);
  const s2 = await state();
  for (const id of ["d0", "d2", "d4"]) assert.equal(presence(s2, id), "online", `${id} online after reconnect`);
  clearInterval(pinger);
  all.forEach((d) => d.close());
});

test("teardown_leaves_nothing", { skip: !TEARDOWN }, async () => {
  const acct = process.env.CLOUDFLARE_ACCOUNT_ID, token = process.env.CLOUDFLARE_API_TOKEN;
  assert.ok(acct && token, "source ~/cloudflare-lc0.env");
  const cf = async (path: string) => {
    if (process.env.LC0_CF_LOG) execFileSync("sh", ["-c", `echo "GET ${path.replace(acct!, ":acct")} (teardown test)" >> "$LC0_CF_LOG"`]);
    return (await fetch(`https://api.cloudflare.com/client/v4${path}`, { headers: { authorization: `Bearer ${token}` } })).json() as any;
  };
  const scripts = await cf(`/accounts/${acct}/workers/scripts`);
  assert.equal(scripts.success, true);
  assert.ok(!scripts.result.some((s: any) => s.id === "loams-cloud-lc0"), "the Worker is gone");
  const nsList = await cf(`/accounts/${acct}/workers/durable_objects/namespaces?per_page=1000`);
  assert.equal(nsList.success, true);
  assert.ok(!nsList.result.some((n: any) => n.script === "loams-cloud-lc0"), "its Durable Object namespaces are gone");
  const clusters = JSON.parse(execFileSync(`${homedir()}/.ti/bin/ti`, ["db", "list-db-clusters", "--db-cluster-type", "starter"], { encoding: "utf8" }));
  assert.ok(!(clusters.clusters ?? []).some((c: any) => c.display_name === "lc0-worker"), "the Starter instance is deleted");
  assert.ok(!existsSync(`${homedir()}/.ti/db_users/${process.env.LC0_CLUSTER_ID ?? "10326096322467809829"}`), "ti's local SQL users are gone");
});
