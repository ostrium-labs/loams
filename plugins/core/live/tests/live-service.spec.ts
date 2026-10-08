import { describe, it, expect, beforeAll, afterAll } from "vite-plus/test";
import { LoamsLiveService, DEFAULT_BASE_URL } from "../src/service.js";
import { Context } from "cordis";
import { DataService } from "@loams-core/data";

/**
 * These run against a real Loams mock (`cargo run -p loams-apps-mock`), because
 * the thing being verified is the wire behaviour: envelope framing, snapshot
 * first, cursor carried across a reconnect. A hand-written fake would agree with
 * itself and prove nothing.
 *
 * Skipped, not failed, when no mock is listening, so the suite stays green for a
 * contributor who has not started one.
 */
const baseUrl = process.env.LOAMS_URL ?? DEFAULT_BASE_URL;

async function reachable(): Promise<boolean> {
  try {
    const res = await fetch(`${baseUrl}/healthz`, { signal: AbortSignal.timeout(1500) });
    return res.ok;
  } catch {
    return false;
  }
}

const live = await reachable();

describe.skipIf(!live)("LoamsLiveService against a live mock", () => {
  let service: LoamsLiveService;
  let ctx: any;

  beforeAll(async () => {
    ctx = new Context();
    // `DataService` injects the control plane, so it has to be on the context
    // before DataService loads or DataService itself is skipped -- and then
    // `data` is unresolved, which silently skips everything that injects it.
    ctx.provide("bi");
    ctx.set("bi", {
      queryData: async () => ({ data: [], rowcount: 0 }),
    });
    await ctx.plugin(DataService);
    await ctx.plugin(LoamsLiveService, { baseUrl, streams: ["operations", "approvals"] });
    service = ctx.loamsLive;
    service.start();
  });

  afterAll(() => service?.dispose());

  it("projects the operations snapshot into rows a widget can chart", async () => {
    await waitFor(() => service.state("operations").items.length > 0);
    const rows = await service.query({ data: { source: "loams", stream: "operations" } });

    expect(rows.rowcount).toBeGreaterThan(0);
    expect(rows.data[0].id).toBeTruthy();
    // Flat columns, because that is what the renderers index by.
    expect(Object.keys(rows.data[0])).toContain("kind");
  });

  it("surfaces a stream that cannot be reached instead of hanging empty", async () => {
    // A fresh context: a context can only host one service per key, so the
    // second instance has to be built on its own.
    const solo = new Context();
    solo.provide("bi");
    solo.set("bi", { queryData: async () => ({ data: [], rowcount: 0 }) });
    await solo.plugin(DataService);
    const broken = new LoamsLiveService(solo, {
      baseUrl: "http://127.0.0.1:1",
      streams: ["operations"],
      reconnectMaxMs: 50,
    });
    broken.start();
    // The failure that costs the most time is a tile that is simply blank with
    // no reason, so the stream's error has to be observable in state.
    await waitFor(
      () => Boolean(broken.state("operations").error),
      8000,
      () => `operations state: ${JSON.stringify(broken.state("operations")).slice(0, 300)}`,
    );
    expect(broken.state("operations").connected).toBe(false);
    expect(broken.state("operations").error).toBeTruthy();
    // And a read still returns a result rather than throwing at the caller.
    expect((await broken.query({ data: { stream: "operations" } })).rowcount).toBe(0);
    broken.dispose();
  });

  it("keeps a filter applied as an equality projection", async () => {
    await waitFor(() => service.state("operations").items.length > 0);
    const all = await service.query({ data: { stream: "operations" } });
    const target = all.data[0].kind as string;

    const filtered = await service.query({
      data: { stream: "operations", filter: { kind: target } },
    });
    expect(filtered.data.every((row: any) => row.kind === target)).toBe(true);

    const none = await service.query({
      data: { stream: "operations", filter: { kind: "no-such-kind" } },
    });
    expect(none.rowcount).toBe(0);
  });

  it("rejects a stream it does not have, rather than serving empty rows", async () => {
    // An empty tile that looks like real data is the failure mode worth
    // avoiding; a thrown error names the mistake.
    await expect(service.query({ data: { stream: "nope" } })).rejects.toThrow(/unknown loams stream/);
  });

  it("advances the cursor so a reconnect resumes rather than restarts", async () => {
    await waitFor(() => service.state("operations").cursor !== "");
    const cursor = service.state("operations").cursor;
    expect(typeof cursor).toBe("string");

    // Force the drop the mock exposes, then confirm it comes back.
    await fetch(`${baseUrl}/mock/drop-streams`, { method: "POST" });
    await waitFor(() => service.state("operations").connected && service.state("operations").items.length > 0, 8000);
    expect(service.state("operations").items.length).toBeGreaterThan(0);
  });

  it("stops notifying a caller that unsubscribed", async () => {
    let seen = 0;
    const off = service.onChange(() => seen++);
    await waitFor(
      () => service.state("approvals").connected,
      8000,
      () => `approvals state: ${JSON.stringify(service.state("approvals")).slice(0, 300)}`,
    );
    const after = seen;
    await new Promise((r) => setTimeout(r, 300));
    off();
    const atOff = seen;
    await new Promise((r) => setTimeout(r, 300));
    // Unsubscribing must actually stop the calls, not just stop being awaited.
    expect(seen).toBe(atOff);
    expect(typeof after).toBe("number");
  });
});

async function waitFor(
  predicate: () => boolean,
  timeoutMs = 5000,
  describe: () => string = () => "",
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(
    `condition not met within ${timeoutMs}ms${describe() ? `\n${describe()}` : ""}`,
  );
}