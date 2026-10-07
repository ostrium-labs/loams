import { describe, it, expect, beforeAll } from "vite-plus/test";
import { LoamsLiveService } from "../src/service.js";
import { Context } from "cordis";
import { DataService } from "@loams-plugins/plugin-data";

/**
 * What the live streams actually deliver, as columns.
 *
 * The dashboard widgets name columns -- `kind`, `state`, `parent_id` -- and a
 * column that is not there does not fail loudly: the chart comes up empty and the
 * derived graph comes up with no edges. So this asserts the shape the seeded
 * dashboard depends on, against the real mock.
 */
const baseUrl = process.env.LOAMS_URL ?? "http://127.0.0.1:8084";

async function reachable(): Promise<boolean> {
  try {
    return (await fetch(`${baseUrl}/healthz`, { signal: AbortSignal.timeout(1500) })).ok;
  } catch {
    return false;
  }
}

const live = await reachable();

describe.skipIf(!live)("the columns a Loams-live dashboard widget names", () => {
  let service: LoamsLiveService;
  let ctx: Context;

  beforeAll(async () => {
    ctx = new Context();
    ctx.provide("controlPlane");
    ctx.set("controlPlane", { queryData: async () => ({ data: [], rowcount: 0 }) });
    await ctx.plugin(DataService);
    await ctx.plugin(LoamsLiveService, { baseUrl, streams: ["operations", "approvals"] });
    service = ctx.loamsLive;
    service.start();
    await waitFor(() => service.state("operations").items.length > 0);
  });

  afterAll(() => service?.dispose());

  it("delivers operations rows carrying the columns the tile reads", async () => {
    const rows = await service.query({ data: { stream: "operations" } });
    const first = rows.data[0] as Record<string, unknown>;
    for (const column of ["id", "kind", "state"]) {
      expect(Object.keys(first), `operations rows are missing \`${column}\``).toContain(column);
    }
  });

  it("delivers no parent pointer on operations, so nothing derives a topology from it", async () => {
    // `Operation` has no parent field in the proto. A derived graph keyed on one
    // would draw a set of unrelated nodes, which reads as a working topology on
    // screen. Asserted so a future proto change that adds one is a deliberate
    // decision to re-point the widget, not a silent surprise.
    const rows = await service.query({ data: { stream: "operations" } });
    expect(Object.keys(rows.data[0] as object)).not.toContain("parent_id");
  });

  it("delivers approvals rows carrying the columns the tiles read", async () => {
    await waitFor(() => service.state("approvals").items.length > 0);
    const rows = await service.query({ data: { stream: "approvals" } });
    const first = rows.data[0] as Record<string, unknown>;
    // `widget-live-approvals` charts state; `widget-live-topology` derives node
    // ids from operationId, which is the one real link to the operations stream.
    for (const column of ["id", "operationId", "kind", "state"]) {
      expect(Object.keys(first), `approvals rows are missing \`${column}\``).toContain(column);
    }
  });

  it("gives the derived graph distinct node ids", async () => {
    // Keyed on operationId: two approvals gating one operation collapse to one
    // node. That is the intent -- the graph answers "what is waiting", not "how
    // many requests" -- but it has to actually collapse, or the ids collide.
    await waitFor(() => service.state("approvals").items.length > 0);
    const rows = await service.query({ data: { stream: "approvals" } });
    const ids = rows.data.map((r: any) => r.operationId);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

async function waitFor(predicate: () => boolean, timeoutMs = 8000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`condition not met within ${timeoutMs}ms`);
}