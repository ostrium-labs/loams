import { describe, it, expect, vi, beforeEach, afterEach } from "vite-plus/test";
import { Context } from "cordis";
import {
  ITSAPLAN_ABSENT_ENDPOINTS,
  ITSAPLAN_CHART_MAX_ROWS,
  ITSAPLAN_RATE_LIMIT_PER_SECOND,
  ItsAPlanAdapterService,
  ItsAPlanApiError,
  burnupProjectedDates,
  isDenseSeries,
  isSparseSeries,
  itsaplanLoader,
  itsaplanManifest,
  unbucketedAgentRuns,
} from "../src/service.js";
import type {
  ItsAPlanAgentRunStats,
  ItsAPlanBurnup,
  ItsAPlanConfig,
  ItsAPlanDashboard,
} from "../src/types.js";
import type { AgentSkillContext, PluginRuntime } from "@loams-core/host";

type MockedFetch = ReturnType<typeof vi.fn<typeof fetch>>;

const BASE = "https://plan.example.test";
const KEY = "itp_abc123";

/** `PluginLoader.skills` receives a runtime its handlers never read. */
const RUNTIME_STUB = {} as PluginRuntime;

/** Always JSON — a bare-string body must still be valid JSON. */
function reply(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * The URL a recorded fetch was called with.
 *
 * The client always passes a string, but the mocked signature is
 * `RequestInfo | URL`, so this narrows properly instead of relying on
 * `String(...)` default object formatting.
 */
function urlString(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof URL) return value.href;
  if (value instanceof Request) return value.url;
  throw new Error(`unexpected fetch input: ${typeof value}`);
}

function urlOf(mock: MockedFetch, index = 0): URL {
  return new URL(urlString(mock.mock.calls[index]?.[0]));
}

/** A header of the nth request, by name. */
function headerOf(mock: MockedFetch, name: string, index = 0): string | null {
  const init = mock.mock.calls[index]?.[1] as RequestInit | undefined;
  return new Headers(init?.headers).get(name);
}

/** The path (no query) of the nth request. */
function pathOf(mock: MockedFetch, index = 0): string {
  return urlOf(mock, index).pathname;
}

describe("ItsAPlanAdapterService", () => {
  let service: ItsAPlanAdapterService;
  let fetchMock: MockedFetch;

  function makeService(overrides: Partial<ItsAPlanConfig> = {}): ItsAPlanAdapterService {
    return new ItsAPlanAdapterService(new Context(), {
      baseUrl: BASE,
      apiKey: KEY,
      ...overrides,
    });
  }

  beforeEach(() => {
    const stub = vi.fn();
    vi.stubGlobal("fetch", stub);
    fetchMock = stub as unknown as MockedFetch;
    service = makeService();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /* ------------------------------------------------------------------ */
  /* Routes and auth                                                    */
  /* ------------------------------------------------------------------ */

  it("targets the API server root with no /api/v1 prefix", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.listIssues("MKT");

    expect(pathOf(fetchMock)).toBe("/projects/MKT/issues");
    expect(urlString(fetchMock.mock.calls[0]?.[0])).not.toContain("/api/v1");
  });

  it("sends the key as x-api-key, not as a Bearer token", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.listIssues("MKT");

    expect(headerOf(fetchMock, "x-api-key")).toBe(KEY);
    expect(headerOf(fetchMock, "Authorization")).toBeNull();
  });

  it("sends x-api-key on every request", async () => {
    fetchMock.mockImplementation(async () =>
      reply({ open: 1, inProgress: 1, backlog: 0, overdue: 0, unassigned: 0, closedLast7d: 0 }),
    );

    await service.health();
    await service.getStats("MKT");

    expect(fetchMock.mock.calls.length).toBe(2);
    for (const call of fetchMock.mock.calls) {
      const init = call[1] as RequestInit | undefined;
      expect(new Headers(init?.headers).get("x-api-key")).toBe(KEY);
    }
  });

  it("uses absolute analytics paths under /projects/:key/analytics", async () => {
    fetchMock.mockImplementation(async () => reply([]));

    await service.getPulse("MKT");
    expect(pathOf(fetchMock, 0)).toBe("/projects/MKT/analytics/pulse");

    fetchMock.mockClear();
    await service.getThroughput("MKT");
    expect(pathOf(fetchMock)).toBe("/projects/MKT/analytics/throughput");
  });

  it("percent-encodes a project key into the path", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.listIssues("a/b c");

    expect(pathOf(fetchMock)).toBe("/projects/a%2Fb%20c/issues");
  });

  it("refuses to construct without a key", () => {
    expect(() => makeService({ apiKey: "" })).toThrow(/api-key\/create/);
  });

  /* ------------------------------------------------------------------ */
  /* Errors: {error, code?} is the only uniform shape                  */
  /* ------------------------------------------------------------------ */

  it("raises on an {error} body returned with a 2xx status", async () => {
    fetchMock.mockResolvedValueOnce(reply({ error: "project not found", code: "NOT_FOUND" }, 200));

    const error = await service.getStats("NOPE").catch((err: unknown) => err);

    expect(error).toBeInstanceOf(ItsAPlanApiError);
    expect((error as ItsAPlanApiError).code).toBe("NOT_FOUND");
    expect((error as ItsAPlanApiError).status).toBe(200);
    expect((error as Error).message).toMatch(/project not found/);
  });

  it("raises on an {error} body carried by a real 4xx", async () => {
    fetchMock.mockResolvedValueOnce(reply({ error: "cursor is invalid" }, 400));

    const error = await service.getActivity("MKT").catch((err: unknown) => err);

    expect(error).toBeInstanceOf(ItsAPlanApiError);
    expect((error as ItsAPlanApiError).status).toBe(400);
    expect((error as Error).message).toMatch(/cursor is invalid/);
  });

  it("never surfaces the generic 500 body text as a cause", async () => {
    fetchMock.mockResolvedValueOnce(reply({ error: "Internal Server Error" }, 500));

    const error = await service.getStats("MKT").catch((err: unknown) => err);

    expect(error).toBeInstanceOf(ItsAPlanApiError);
    expect((error as Error).message).toMatch(/failed with 500/);
    // The server's own wording is deliberately not repeated as if it explained anything.
    expect((error as Error).message).not.toMatch(/Internal Server Error/);
    expect((error as Error).message).toMatch(/generic by design/);
  });

  it("labels a 403/404 on an analytics route as a configuration problem", async () => {
    fetchMock.mockResolvedValueOnce(new Response("", { status: 403 }));

    const error = await service.getStats("MKT").catch((err: unknown) => err);

    expect(error).toBeInstanceOf(ItsAPlanApiError);
    expect((error as ItsAPlanApiError).isConfiguration).toBe(true);
    expect((error as Error).message).toMatch(/dashboards/);
  });

  /* ------------------------------------------------------------------ */
  /* Bare arrays: no envelope anywhere                                  */
  /* ------------------------------------------------------------------ */

  it("parses /breakdown as a BARE ARRAY and keeps `key` a string", async () => {
    fetchMock.mockResolvedValueOnce(
      reply([
        { key: "12::text", label: "In Progress", count: 7, color: "#4f8ef7" },
        { key: "none", label: "No status", count: 2, color: null },
      ]),
    );

    const items = await service.getBreakdown("MKT", "status");

    expect(Array.isArray(items)).toBe(true);
    expect(items).toHaveLength(2);
    // TRAP: `key` is a stringified `${columnId}::text` (or a 'none' sentinel),
    // never a number. Coercing it with Number() yields NaN for a composite key.
    expect(typeof items[0]?.key).toBe("string");
    expect(items[0]?.key).toBe("12::text");
    expect(items[1]?.key).toBe("none");
    expect(Number.isNaN(Number(items[0]?.key))).toBe(true);
    expect(urlOf(fetchMock).searchParams.get("by")).toBe("status");
  });

  it("raises when a bare-array route answers with an envelope instead", async () => {
    // The classic silent bug: parsing `{items:[...]}` as a bare array yields
    // nothing at all. The adapter refuses rather than returning an empty report.
    fetchMock.mockImplementation(async () =>
      reply({ items: [{ key: "a", label: "A", count: 1, color: null }] }),
    );

    await expect(service.getBreakdown("MKT", "status")).rejects.toBeInstanceOf(ItsAPlanApiError);
    await expect(service.getBreakdown("MKT", "status")).rejects.toThrow(/NO list envelope/);
  });

  it("parses /agent-runs and /agent-workload as bare arrays", async () => {
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const path = new URL(urlString(input)).pathname;
      if (path.endsWith("agent-runs")) {
        return reply([
          {
            id: "r1",
            status: "success",
            trigger: "mention",
            agentId: "a1",
            agentName: "ada",
            issueId: "i1",
            issueSequence: 42,
            lastError: null,
            createdAt: "2026-09-01T10:00:00.000Z",
          },
        ]);
      }
      return reply([
        {
          agentId: "a1",
          agentName: "ada",
          kind: "worker",
          delegatedOpen: 4,
          runsTotal: 9,
          runsSuccess: 8,
          runsFailed: 1,
        },
      ]);
    });

    const runs = await service.getAgentRuns("MKT", { status: "success", limit: 10 });
    expect(runs).toHaveLength(1);
    expect(runs[0]?.trigger).toBe("mention");
    expect(urlOf(fetchMock).searchParams.get("status")).toBe("success");

    const workload = await service.getAgentWorkload("MKT");
    expect(workload[0]?.agentName).toBe("ada");
    expect(workload[0]?.delegatedOpen).toBe(4);
  });

  it("parses /issues as a bare array and sends CSV filters as comma lists", async () => {
    fetchMock.mockResolvedValueOnce(
      reply([
        {
          id: "i1",
          sequenceNumber: 42,
          identifier: "MKT-42",
          title: "Ship the thing",
          columnId: "c1",
          typeId: "t1",
          labelIds: ["l1", "l2"],
          archived: false,
        },
      ]),
    );

    const issues = await service.listIssues("MKT", {
      labelIds: "l1,l2",
      assigneeUserId: "u1",
      limit: 25,
    });

    expect(issues[0]?.identifier).toBe("MKT-42");
    const params = urlOf(fetchMock).searchParams;
    expect(params.get("labelIds")).toBe("l1,l2");
    expect(params.get("assigneeUserId")).toBe("u1");
    expect(params.get("limit")).toBe("25");
  });

  /* ------------------------------------------------------------------ */
  /* Density: pulse is dense, throughput is sparse                      */
  /* ------------------------------------------------------------------ */

  it("tags /pulse as DENSE and preserves the server's order and every bucket", async () => {
    // A dense axis: the zeros are present because the server generates them.
    fetchMock.mockResolvedValueOnce(
      reply([
        { label: "2026-09-01", count: 3 },
        { label: "2026-09-02", count: 0 },
        { label: "2026-09-03", count: 5 },
      ]),
    );

    const series = await service.getPulse("MKT", { unit: "day" });

    expect(series.density).toBe("dense");
    expect(isDenseSeries(series)).toBe(true);
    expect(isSparseSeries(series)).toBe(false);
    expect(series.points.map((point) => point.count)).toEqual([3, 0, 5]);
    // Rendered order is authoritative: no re-sorting, no date maths.
    expect(series.points.map((point) => point.label)).toEqual([
      "2026-09-01",
      "2026-09-02",
      "2026-09-03",
    ]);
  });

  it("tags /throughput as SPARSE and leaves the absent weeks absent", async () => {
    // A sparse series: the empty week is simply missing.
    fetchMock.mockResolvedValueOnce(
      reply([
        { week: "2026-08-24", created: 4, closed: 2 },
        { week: "2026-09-07", created: 7, closed: 5 },
      ]),
    );

    const series = await service.getThroughput("MKT", { weeks: 4 });

    expect(series.density).toBe("sparse");
    expect(isSparseSeries(series)).toBe(true);
    expect(isDenseSeries(series)).toBe(false);
    // 2026-08-31 is genuinely absent. The adapter does NOT gap-fill it, because
    // a synthesised zero is a claim the server did not make.
    expect(series.weeks.map((week) => week.week)).toEqual(["2026-08-24", "2026-09-07"]);
    expect(series.weeks).toHaveLength(2);
    expect(urlOf(fetchMock).searchParams.get("weeks")).toBe("4");
  });

  it("keeps the two series types distinguishable even when both are empty", async () => {
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const path = new URL(urlString(input)).pathname;
      return reply(path.endsWith("pulse") ? [] : []);
    });

    const pulse = await service.getPulse("MKT");
    const throughput = await service.getThroughput("MKT");

    expect(pulse.density).toBe("dense");
    expect(throughput.density).toBe("sparse");
  });

  /* ------------------------------------------------------------------ */
  /* Burnup forecast                                                    */
  /* ------------------------------------------------------------------ */

  function burnupOf(
    forecast: Partial<ItsAPlanBurnup["forecast"]>,
    targetDate: string | null = null,
  ): ItsAPlanBurnup {
    return {
      days: [{ date: "2026-09-01", scope: 40, started: 40, completed: 12 }],
      forecast: {
        windowDays: 28,
        velocityPerDay: 0.5,
        scopeGrowthPerDay: 0.1,
        remaining: 28,
        projectedScope: 54,
        projectedDate: "2026-11-30",
        optimisticDate: "2026-11-01",
        pessimisticDate: "2026-12-31",
        ...forecast,
      },
      targetDate,
    };
  }

  it("reads all three projected dates when the forecast made them", () => {
    const burnup = burnupOf({}, "2026-10-31");

    const dates = burnupProjectedDates(burnup);

    expect(dates).toEqual({
      projectedDate: "2026-11-30",
      optimisticDate: "2026-11-01",
      pessimisticDate: "2026-12-31",
    });
  });

  it("returns null when all three projected dates are null together", () => {
    // Nothing closed in the window, or nothing remains: the server nulls the
    // whole projection, including the optimistic/pessimistic PAIR.
    const burnup = burnupOf({
      projectedDate: null,
      optimisticDate: null,
      pessimisticDate: null,
      velocityPerDay: 0,
      remaining: 0,
    });

    expect(burnupProjectedDates(burnup)).toBeNull();
    // The pair is null WITH the projection; reading it independently would
    // invent a date the server declined to give.
    expect(burnup.forecast.optimisticDate).toBeNull();
    expect(burnup.forecast.pessimisticDate).toBeNull();
  });

  it("surfaces a null targetDate without treating it as missing data", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        days: [{ date: "2026-09-01", scope: 40, started: 40, completed: 12 }],
        forecast: {
          windowDays: 28,
          velocityPerDay: 0.5,
          scopeGrowthPerDay: 0.1,
          remaining: 28,
          projectedScope: 54,
          projectedDate: "2026-11-30",
          optimisticDate: "2026-11-01",
          pessimisticDate: "2026-12-31",
        },
        targetDate: null,
      }),
    );

    const burnup = await service.getBurnup("MKT", { days: 90, forecastWeeks: 4 });

    expect(burnup.targetDate).toBeNull();
    expect(burnup.days).toHaveLength(1);
    expect(service.burnupDates(burnup)?.projectedDate).toBe("2026-11-30");
    const params = urlOf(fetchMock).searchParams;
    expect(params.get("days")).toBe("90");
    expect(params.get("forecastWeeks")).toBe("4");
  });

  /* ------------------------------------------------------------------ */
  /* agent-run-stats is NOT a partition                                 */
  /* ------------------------------------------------------------------ */

  it("reads agent-run-stats without asserting total === success+failed+pending", async () => {
    // A run with a status outside the three buckets increments `total` and no
    // bucket, so total legitimately EXCEEDS the sum.
    fetchMock.mockResolvedValueOnce(reply({ total: 10, success: 4, failed: 2, pending: 1 }));

    const stats = await service.getAgentRunStats("MKT", { days: 30 });

    expect(stats).toEqual({ total: 10, success: 4, failed: 2, pending: 1 });
    // The adapter reports the excess rather than normalising it away.
    expect(unbucketedAgentRuns(stats)).toBe(3);
    expect(stats.total).toBeGreaterThan(stats.success + stats.failed + stats.pending);
  });

  it("unbucketedAgentRuns is zero for a response that happens to sum exactly", () => {
    const stats: ItsAPlanAgentRunStats = { total: 7, success: 4, failed: 2, pending: 1 };
    expect(stats.success + stats.failed + stats.pending).toBe(stats.total);
    expect(unbucketedAgentRuns(stats)).toBe(0);
  });

  it("unbucketedAgentRuns never goes negative", () => {
    const stats: ItsAPlanAgentRunStats = { total: 1, success: 4, failed: 2, pending: 1 };
    expect(unbucketedAgentRuns(stats)).toBe(0);
  });

  it("reads webhook-stats with the same overwrite caveat intact", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        total: 9,
        success: 3,
        failed: 1,
        pending: 0,
        activeWebhooks: 5,
        disabledWebhooks: 2,
      }),
    );

    const stats = await service.getWebhookStats("MKT");

    expect(stats.activeWebhooks).toBe(5);
    expect(stats.total).toBeGreaterThan(stats.success + stats.failed + stats.pending);
  });

  /* ------------------------------------------------------------------ */
  /* Cursor pagination                                                  */
  /* ------------------------------------------------------------------ */

  function activityPage(count: number, next: { ts: string; id: string } | null): unknown {
    return {
      items: Array.from({ length: count }, (_, index) => ({
        id: `${next?.id ?? "seed"}-${index}`,
        issueId: `i${index}`,
        issueSequence: index,
        issueTitle: `event ${index}`,
        kind: "issue.created",
        actorUserId: "u1",
        actorName: "ada",
        body: null,
        action: "created",
        payload: {},
        createdAt: next?.ts ?? "2026-09-01T10:00:00.000Z",
      })),
      nextCursor: next,
    };
  }

  it("reads the cursor feed's {items, nextCursor} and serialises the cursor back", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(activityPage(2, { ts: "2026-09-01T10:00:00.000Z", id: "42" })),
    );

    const page = await service.getActivity("MKT", { limit: 2 });

    expect(page.items).toHaveLength(2);
    expect(page.nextCursor).toEqual({ ts: "2026-09-01T10:00:00.000Z", id: "42" });
    expect(urlOf(fetchMock).searchParams.get("limit")).toBe("2");
    expect(urlOf(fetchMock).searchParams.has("cursor")).toBe(false);

    fetchMock.mockClear();
    fetchMock.mockResolvedValueOnce(reply(activityPage(0, null)));
    await service.getActivity("MKT", { cursor: page.nextCursor });
    expect(urlOf(fetchMock).searchParams.get("cursor")).toBe("2026-09-01T10:00:00.000Z,42");
  });

  it("reports nextCursor as null on the last page", async () => {
    fetchMock.mockResolvedValueOnce(reply(activityPage(1, null)));

    const page = await service.getActivity("MKT");

    expect(page.nextCursor).toBeNull();
  });

  it("passes the CSV issueIds filter through and documents the empty-feed case", async () => {
    // An empty match yields an EMPTY feed. That is documented behaviour, and the
    // response is indistinguishable from "end of feed" — which is exactly why the
    // adapter does not invent items or call it a data problem.
    fetchMock.mockResolvedValueOnce(reply({ items: [], nextCursor: null }));

    const page = await service.getActivity("MKT", { issueIds: "999,1000" });

    expect(page.items).toEqual([]);
    expect(page.nextCursor).toBeNull();
    expect(urlOf(fetchMock).searchParams.get("issueIds")).toBe("999,1000");
  });

  it("walks the cursor feed to the end and stops on a non-advancing cursor", async () => {
    fetchMock.mockImplementation(async () =>
      reply(activityPage(1, { ts: "2026-09-01T10:00:00.000Z", id: "7" })),
    );

    const stuck = await service.getAllActivity("MKT", { maxPages: 5 });
    // The same cursor comes back every time, so the walk must terminate.
    expect(stuck).toHaveLength(2);
    expect(fetchMock.mock.calls.length).toBe(2);
  });

  /* ------------------------------------------------------------------ */
  /* Dashboards: LIST ONLY, layout is unknown                           */
  /* ------------------------------------------------------------------ */

  it("lists dashboards from the collection route and never requests one by id", async () => {
    const dashboards: ItsAPlanDashboard[] = [
      {
        id: "d1",
        projectId: "p1",
        name: "Delivery",
        icon: "chart",
        // `layout` is untyped jsonb the server never inspects.
        layout: [{ id: "w1", type: "pulse", x: 0, y: 0, w: 6, h: 4 }],
        position: 0,
        createdAt: "2026-09-01T10:00:00.000Z",
      },
    ];
    fetchMock.mockResolvedValueOnce(reply(dashboards));

    const result = await service.listDashboards("MKT");

    expect(pathOf(fetchMock)).toBe("/projects/MKT/dashboards");
    expect(result).toHaveLength(1);
    expect(result[0]?.layout).toEqual([{ id: "w1", type: "pulse", x: 0, y: 0, w: 6, h: 4 }]);

    // No single-dashboard route exists, so no method may ask for one.
    expect(ITSAPLAN_ABSENT_ENDPOINTS).toContain(
      "GET /projects/:projectKey/dashboards/:dashboardId",
    );
    expect(typeof (service as unknown as Record<string, unknown>)["getDashboard"]).toBe(
      "undefined",
    );
    for (const name of Object.getOwnPropertyNames(Object.getPrototypeOf(service))) {
      expect(String(name)).not.toMatch(/^getDashboard$/);
    }
  });

  it("accepts a layout that is not interpretable, because the server never inspects it", async () => {
    fetchMock.mockResolvedValueOnce(
      reply([
        {
          id: "d2",
          projectId: "p1",
          name: "Future",
          layout: { anything: ["at", "all"] },
          createdAt: "2026-09-01T10:00:00.000Z",
        },
      ]),
    );

    const result = await service.listDashboards("MKT");

    // `layout` is `unknown`, so this is carried through untouched rather than
    // forced into a widget catalogue the API does not define.
    expect(result[0]?.layout).toEqual({ anything: ["at", "all"] });
  });

  /* ------------------------------------------------------------------ */
  /* Chart spec: validated and echoed, nothing stored                   */
  /* ------------------------------------------------------------------ */

  it("POSTs a chart spec to be validated and echoes it back", async () => {
    const spec = {
      type: "bar" as const,
      x: "state",
      series: [{ key: "count", label: "Issues" }],
      data: [{ state: "open", count: 4 }],
    };
    fetchMock.mockResolvedValueOnce(reply(spec));

    const echoed = await service.validateChartSpec("MKT", spec);

    const requestInit = fetchMock.mock.calls[0]?.[1] as RequestInit | undefined;
    expect(requestInit?.method).toBe("POST");
    expect(pathOf(fetchMock)).toBe("/projects/MKT/charts");
    expect(echoed).toEqual(spec);
  });

  it("refuses an oversized chart spec before spending a round trip", async () => {
    const data = Array.from({ length: ITSAPLAN_CHART_MAX_ROWS + 1 }, () => ({ count: 1 }));

    await expect(
      service.validateChartSpec("MKT", { type: "bar", x: "state", series: [], data }),
    ).rejects.toThrow(/at most 500/);
    expect(fetchMock.mock.calls.length).toBe(0);
  });

  it("never issues a non-GET other than the chart-spec validation", async () => {
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const path = new URL(urlString(input)).pathname;
      return reply(path.endsWith("/charts") ? { type: "bar", x: "a", series: [], data: [] } : []);
    });

    await service.getStats("MKT").catch(() => undefined);
    await service.listIssues("MKT");
    await service.listDashboards("MKT");
    await service.validateChartSpec("MKT", { type: "bar", x: "a", series: [], data: [] });

    const methods = fetchMock.mock.calls.map(
      (call) => (call[1] as RequestInit | undefined)?.method ?? "GET",
    );
    expect(methods).toEqual(["GET", "GET", "GET", "POST"]);
  });

  /* ------------------------------------------------------------------ */
  /* Rate limit                                                         */
  /* ------------------------------------------------------------------ */

  it("exposes the documented 100 req/s per-key budget", () => {
    expect(ITSAPLAN_RATE_LIMIT_PER_SECOND).toBe(100);
    expect(service.rateLimitPerSecond).toBe(100);
  });
});

/* -------------------------------------------------------------------------- */
/* Manifest                                                                     */
/* -------------------------------------------------------------------------- */

describe("itsaplan manifest", () => {
  it("declares the manifest contract", () => {
    expect(itsaplanManifest.id).toBe("itsaplan");
    expect(itsaplanManifest.uiPath).toBe("/plugins/itsaplan");
    expect(itsaplanManifest.upstream).toEqual({ product: "It's a Plan", envPrefix: "ITSAPLAN" });
    expect(itsaplanManifest.agent?.skills.length).toBeGreaterThan(0);
    for (const skill of itsaplanManifest.agent?.skills ?? []) {
      expect(skill.id).toMatch(/^[a-zA-Z][a-zA-Z0-9]*$/);
      expect(skill.description.length).toBeGreaterThan(0);
    }
  });

  it("pairs every declared skill with a loader handler", () => {
    const declared = (itsaplanManifest.agent?.skills ?? []).map((skill) => skill.id);
    const handled = (itsaplanLoader.skills?.(RUNTIME_STUB) ?? []).map((skill) => skill.id);
    expect(handled).toEqual(declared);
  });

  it("notes the AGPL position, the x-api-key route and manual key provisioning", () => {
    const description = itsaplanManifest.agent?.description ?? "";
    expect(description).toMatch(/x-api-key/);
    expect(description).toMatch(/AGPL-3\.0/);
    expect(description).toMatch(/no It'sAPlan code|only its HTTP API/);
  });

  it("does not claim web-analytics concepts this product does not have", () => {
    const allSkillText = (itsaplanManifest.agent?.skills ?? [])
      .map((skill) => `${skill.name} ${skill.description}`)
      .join(" ")
      .toLowerCase();
    for (const forbidden of ["pageview", "referrer", "session source", "bounce rate", "geo"]) {
      expect(allSkillText).not.toContain(forbidden);
    }
  });

  it("requires `by` on the breakdown skill rather than defaulting a dimension", async () => {
    const handler = (itsaplanLoader.skills?.(RUNTIME_STUB) ?? []).find(
      (skill) => skill.id === "getBreakdown",
    );
    expect(handler).toBeDefined();
    if (!handler) throw new Error("getBreakdown skill handler is missing");
    // The `by` check runs before `ctx.itsaplan` is touched, so an empty context
    // is enough — and a default dimension would let the skill through.
    const api = { ctx: new Context() } as unknown as AgentSkillContext;
    await expect(handler.handle({ projectKey: "MKT" }, api)).rejects.toThrow(/"by" is required/);
  });
});
