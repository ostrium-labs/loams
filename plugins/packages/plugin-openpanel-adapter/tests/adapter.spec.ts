import { describe, it, expect, beforeEach, afterEach, vi } from "vite-plus/test";
import { Context } from "cordis";
import {
  OPENPANEL_CLIENT_ID_HEADER,
  OPENPANEL_CLIENT_SECRET_HEADER,
  OPENPANEL_CLIENT_ID_PATTERN,
  OPENPANEL_IGNORED_INSIGHTS_PARAMS,
  OPENPANEL_INSIGHTS_LIMIT_MAX,
  OpenPanelAdapterService,
  OpenPanelAuthError,
  OpenPanelFilter,
  OpenPanelValidationError,
  SlidingWindowThrottle,
  assertOpenPanelClientId,
  describeOpenPanelAuthFailure,
  describeOpenPanelTimeWindow,
  isOpenPanelClientId,
  openPanelAuthHeaders,
  parseClickHouseDate,
  resolveOpenPanelBaseUrl,
  serializeOpenPanelFilters,
} from "../src/index.js";
import { openPanelManifest } from "../src/index.js";
import { buildQuery } from "@loams-core/http";

/**
 * The cases here target the two ways OpenPanel fails SILENTLY: a bare response
 * mistaken for an enveloped one, and a date parsed with the wrong assumption
 * about its timezone. Neither produces an error anywhere.
 */

const VALID_CLIENT_ID = "0195f2ae-7c1a-7c2b-9f3d-4e5a6b7c8d9e";

interface Captured {
  url: string;
  headers: Record<string, string>;
}

interface Harness {
  service: OpenPanelAdapterService;
  captured: Captured[];
  restore(): void;
}

function makeService(
  overrides: {
    clientId?: string;
    clientSecret?: string;
    baseUrl?: string;
    apiPrefix?: string;
  } = {},
): Harness {
  const captured: Captured[] = [];
  const original = globalThis.fetch;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((value, key) => {
      headers[key.toLowerCase()] = value;
    });
    captured.push({ url, headers });
    return new Response(JSON.stringify(respond(new URL(url).pathname)), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;

  const service = new OpenPanelAdapterService(new Context(), {
    baseUrl: overrides.baseUrl ?? "https://analytics.test",
    clientId: overrides.clientId ?? VALID_CLIENT_ID,
    clientSecret: overrides.clientSecret ?? "shh",
    ...(overrides.apiPrefix !== undefined ? { apiPrefix: overrides.apiPrefix } : {}),
  });
  return { service, captured, restore: () => (globalThis.fetch = original) };
}

function respond(pathname: string): unknown {
  if (pathname === "/api/healthcheck") return { status: "ok" };
  if (pathname === "/api/export/events") {
    return {
      meta: { count: 2, totalCount: 12, pages: 2, current: 1 },
      data: [
        {
          id: "e1",
          name: "purchase",
          deviceId: "d",
          profileId: "p",
          projectId: "proj",
          sessionId: "s",
          properties: {},
          createdAt: "2026-08-07T12:34:56.000Z",
          revenue: 9.99,
        },
        {
          id: "e2",
          name: "view",
          deviceId: "d",
          profileId: "p",
          projectId: "proj",
          sessionId: "s",
          properties: {},
          createdAt: "2026-08-07T12:35:00.000Z",
        },
      ],
    };
  }
  if (pathname === "/api/insights/proj/overview") {
    return {
      summary: {
        bounce_rate: 0.42,
        unique_visitors: 100,
        total_sessions: 40,
        avg_session_duration: 125.5,
        total_screen_views: 400,
        views_per_session: 10,
        total_revenue: 20,
      },
      series: [
        {
          date: "2026-08-07T00:00:00.000Z",
          bounce_rate: 0.42,
          unique_visitors: 100,
          total_sessions: 40,
          avg_session_duration: 125.5,
          total_screen_views: 400,
          views_per_session: 10,
          total_revenue: 20,
        },
      ],
      interval: "day",
      startDate: "2026-08-01T00:00:00.000Z",
      endDate: "2026-08-07T00:00:00.000Z",
    };
  }
  if (pathname === "/api/insights/proj/retention") {
    return [{ date: "2026-08-07", active_users: 100, retained_users: 42, retention: 42.0 }];
  }
  if (pathname === "/api/insights/proj/pages/top") {
    return [{ origin: "https://shop.test", path: "/", sessions: 10, pageviews: 20 }];
  }
  if (pathname === "/api/insights/proj/pages/performance") {
    return {
      total_pages: 1,
      shown: 1,
      pages: [
        {
          origin: "https://shop.test",
          path: "/",
          title: "Home",
          sessions: 10,
          pageviews: 20,
          avg_duration: 2.1,
          bounce_rate: 0.5,
          seo_signals: { high_bounce: true, low_engagement: false, good_landing_page: true },
        },
      ],
    };
  }
  if (pathname === "/api/insights/proj/events") {
    // RAW ClickHouse row: snake_case, date with no Z and no milliseconds.
    return [
      {
        id: "raw-1",
        created_at: "2026-08-07 12:34:56",
        profile_id: "p",
        session_id: "s",
        referrer_name: "Google",
        os_version: "15.1",
        browser_version: "121.0",
        event_count: 3,
        is_bounce: false,
        entry_path: "/",
        exit_origin: "https://shop.test",
      },
    ];
  }
  if (pathname === "/api/manage/projects") {
    return { data: [{ id: "proj", createdAt: "2026-01-01T00:00:00.000Z", updatedAt: null }] };
  }
  return [];
}

let harness: Harness | null = null;
beforeEach(() => {
  harness = makeService();
});
afterEach(() => {
  harness?.restore();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("openpanel base URL", () => {
  it("appends /api for the public URL, because Caddy strips it before proxying", () => {
    expect(resolveOpenPanelBaseUrl("https://analytics.test")).toBe("https://analytics.test/api");
  });

  it("does not double a prefix that is already present", () => {
    expect(resolveOpenPanelBaseUrl("https://analytics.test/api")).toBe(
      "https://analytics.test/api",
    );
  });

  it("omits the prefix entirely when pointing straight at the container", () => {
    expect(resolveOpenPanelBaseUrl("http://op-api:3000", "")).toBe("http://op-api:3000");
  });

  it("requests /api/export/events through the public base", async () => {
    await harness!.service.listExportEvents();
    expect(new URL(harness!.captured[0]!.url).pathname).toBe("/api/export/events");
  });
});

describe("openpanel auth headers", () => {
  it("sends exactly two custom headers, lowercase, and no Authorization", async () => {
    await harness!.service.overview("proj");
    const headers = harness!.captured[0]!.headers;
    expect(headers[OPENPANEL_CLIENT_ID_HEADER]).toBe(VALID_CLIENT_ID);
    expect(headers[OPENPANEL_CLIENT_SECRET_HEADER]).toBe("shh");
    // The names must be lowercase on the wire: OpenPanel's middleware reads them.
    expect(OPENPANEL_CLIENT_ID_HEADER).toBe("openpanel-client-id");
    expect(OPENPANEL_CLIENT_SECRET_HEADER).toBe("openpanel-client-secret");
    expect(headers.authorization).toBeUndefined();
  });

  it("builds the header pair with no Bearer and no JWT", () => {
    const headers = openPanelAuthHeaders(VALID_CLIENT_ID, "shh");
    expect(Object.keys(headers).sort()).toEqual(["openpanel-client-id", "openpanel-client-secret"]);
    expect(Object.values(headers).join(" ")).not.toMatch(/bearer|eyJ/i);
  });

  it("fails fast on a client id that is not UUIDv4-shaped, without a network call", () => {
    const before = harness!.captured.length;
    expect(() => assertOpenPanelClientId("not-a-uuid")).toThrow(OpenPanelAuthError);
    expect(() => assertOpenPanelClientId("not-a-uuid")).toThrow(/must be a valid UUIDv4/);
    expect(() => harness!.service.overview("proj")).not.toThrow();
    // And the constructor refuses outright for a bad id, so no request is made.
    expect(
      () =>
        new OpenPanelAdapterService(new Context(), {
          baseUrl: "https://analytics.test",
          clientId: "0195F2AE-7C1A-7C2B-9F3D-4E5A6B7C8D9E",
          clientSecret: "shh",
        }),
    ).toThrow(/lowercase hex/);
    expect(before).toBe(harness!.captured.length);
  });

  it("rejects uppercase, missing dashes and truncated ids", () => {
    expect(isOpenPanelClientId(VALID_CLIENT_ID)).toBe(true);
    expect(isOpenPanelClientId(VALID_CLIENT_ID.toUpperCase())).toBe(false);
    expect(isOpenPanelClientId("0195f2ae7c1a7c2b9f3d4e5a6b7c8d9e")).toBe(false);
    expect(isOpenPanelClientId("0195f2ae-7c1a")).toBe(false);
    expect(OPENPANEL_CLIENT_ID_PATTERN.test(VALID_CLIENT_ID)).toBe(true);
  });

  it("names the write-client-type trap when the id is missing entirely", () => {
    expect(() => assertOpenPanelClientId("")).toThrow(/not `write`/);
  });

  it("surfaces OpenPanel's branchable 401 reason verbatim", () => {
    const body = JSON.stringify({
      error: "Unauthorized",
      message: "Export: Client is not allowed to export",
    });
    expect(describeOpenPanelAuthFailure(401, body)).toBe(
      "openpanel: 401 Export: Client is not allowed to export",
    );
    expect(describeOpenPanelAuthFailure(401, body)).toContain(
      "Export: Client is not allowed to export",
    );
  });

  it("still names every documented reason", () => {
    const reasons = [
      "Export: Client ID must be a valid UUIDv4",
      "Export: Invalid client id",
      "Export: Client has no secret",
      "Export: Client is not allowed to export",
      "Export: Invalid client secret",
    ];
    for (const reason of reasons) {
      const body = JSON.stringify({ error: "Unauthorized", message: reason });
      expect(describeOpenPanelAuthFailure(401, body)).toContain(reason);
    }
  });

  it("rewrites an UpstreamError 401 through the reason extractor", async () => {
    harness!.restore();
    const original = globalThis.fetch;
    globalThis.fetch = (async () =>
      new Response(
        JSON.stringify({ error: "Unauthorized", message: "Export: Invalid client secret" }),
        {
          status: 401,
          headers: { "Content-Type": "application/json" },
        },
      )) as typeof fetch;
    await expect(harness!.service.overview("proj")).rejects.toThrow(
      /Export: Invalid client secret/,
    );
    globalThis.fetch = original;
  });
});

describe("openpanel dual naming conventions", () => {
  it("returns /export rows in camelCase with ISO dates", async () => {
    const page = await harness!.service.listExportEvents();
    const row = page.data[0]!;
    expect(row.createdAt).toBe("2026-08-07T12:34:56.000Z");
    expect(row.deviceId).toBe("d");
    expect(row.sessionId).toBe("s");
    expect(Object.keys(row)).not.toContain("created_at");
  });

  it("returns /insights rows in snake_case with unzoned ClickHouse dates", async () => {
    const rows = await harness!.service.insightEvents("proj");
    const row = rows[0]!;
    expect(row.created_at).toBe("2026-08-07 12:34:56");
    expect(row.profile_id).toBe("p");
    expect(row.referrer_name).toBe("Google");
    expect(row.browser_version).toBe("121.0");
    expect(row.is_bounce).toBe(false);
    expect(row.entry_path).toBe("/");
    // No `Z`, no milliseconds — this is the raw ClickHouse rendering.
    expect(row.created_at as string).not.toMatch(/[Z+]|\.\d{3}/);
  });

  it("never makes one family satisfy the other", async () => {
    const exported = await harness!.service.listExportEvents();
    const raw = await harness!.service.insightEvents("proj");
    const exportKeys = Object.keys(exported.data[0]!);
    const rawKeys = Object.keys(raw[0]!);
    expect(exportKeys).toContain("createdAt");
    expect(exportKeys).not.toContain("created_at");
    expect(rawKeys).toContain("created_at");
    expect(rawKeys).not.toContain("createdAt");
  });
});

describe("openpanel ClickHouse date helper", () => {
  it("parses `YYYY-MM-DD HH:mm:ss` as UTC, not local time", () => {
    const parsed = parseClickHouseDate("2026-08-07 12:34:56");
    expect(parsed.toISOString()).toBe("2026-08-07T12:34:56.000Z");
  });

  it("is UTC regardless of the machine's timezone", () => {
    // The whole bug: `new Date("2026-08-07 12:34:56")` reads as LOCAL time in V8.
    const previous = process.env.TZ;
    process.env.TZ = "Asia/Kolkata";
    try {
      const naive = new Date("2026-08-07 12:34:56");
      const helper = parseClickHouseDate("2026-08-07 12:34:56");
      expect(helper.toISOString()).toBe("2026-08-07T12:34:56.000Z");
      // Demonstrate the divergence the helper exists to prevent.
      expect(naive.getTime()).not.toBe(helper.getTime());
    } finally {
      if (previous === undefined) delete process.env.TZ;
      else process.env.TZ = previous;
    }
  });

  it("handles a midnight value without rolling to the previous day", () => {
    expect(parseClickHouseDate("2026-01-01 00:00:00").toISOString()).toBe(
      "2026-01-01T00:00:00.000Z",
    );
  });

  it("names the two conventions when handed something unparseable", () => {
    expect(() => parseClickHouseDate("not a date")).toThrow(/YYYY-MM-DD HH:mm:ss/);
  });
});

describe("openpanel aggregate dates vs row dates", () => {
  it("parses overview series[].date as plain ISO — it is NOT a ClickHouse string", async () => {
    const overview = await harness!.service.overview("proj");
    const point = overview.series[0]!;
    expect(point.date).toBe("2026-08-07T00:00:00.000Z");
    // No helper, no Z-appending: `new Date` alone is already correct here.
    expect(new Date(point.date).toISOString()).toBe("2026-08-07T00:00:00.000Z");
  });

  it("parses a raw insights row created_at with the UTC helper", async () => {
    const rows = await harness!.service.insightEvents("proj");
    const createdAt = rows[0]!.created_at as string;
    // The wire value has no Z and no milliseconds.
    expect(createdAt).toBe("2026-08-07 12:34:56");
    expect(parseClickHouseDate(createdAt).toISOString()).toBe("2026-08-07T12:34:56.000Z");
  });
});

describe("openpanel unit differences", () => {
  it("keeps overview.avg_session_duration in SECONDS and pages/performance avg_duration in MINUTES", async () => {
    const overview = await harness!.service.overview("proj");
    const performance = await harness!.service.pagesPerformance("proj");

    // Two near-identical field names, one product, two different units. The
    // seconds-valued one is 125.5 SECONDS; the minutes-valued one is 2.1 MINUTES.
    // Converting either by the other's factor is wrong, which is exactly why the
    // unit is documented at both declarations rather than inferred.
    expect(overview.summary.avg_session_duration).toBe(125.5);
    expect(overview.series[0]!.avg_session_duration).toBe(125.5);
    expect(performance.pages[0]!.avg_duration).toBe(2.1);

    // Asserted explicitly so a future refactor that normalises one of them fails here.
    const asSeconds = performance.pages[0]!.avg_duration * 60;
    const asMinutes = overview.summary.avg_session_duration / 60;
    expect(asSeconds).not.toBe(overview.summary.avg_session_duration);
    expect(asMinutes).not.toBe(performance.pages[0]!.avg_duration);
  });
});

describe("openpanel pagination", () => {
  it("uses 1-based `page` plus `limit` on /export/events", async () => {
    const page = await harness!.service.listExportEvents({ page: 2, limit: 500 });
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.get("page")).toBe("2");
    expect(url.searchParams.get("limit")).toBe("500");
    expect(page.meta).toEqual({ count: 2, totalCount: 12, pages: 2, current: 1 });
  });

  it("defaults page to 1 and limit to 50", async () => {
    await harness!.service.listExportEvents();
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.get("page")).toBe("1");
    expect(url.searchParams.get("limit")).toBe("50");
  });

  it("clamps limit to the server's 1..1000", async () => {
    await harness!.service.listExportEvents({ limit: 99999 });
    expect(new URL(harness!.captured[0]!.url).searchParams.get("limit")).toBe("1000");
  });

  it("assumes /insights/* is NOT paginated and sends no page/cursor/offset", async () => {
    await harness!.service.topPages("proj");
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.get("page")).toBeNull();
    expect(url.searchParams.get("cursor")).toBeNull();
    expect(url.searchParams.get("offset")).toBeNull();
  });

  it("caps /insights/* limit at 100, not 1000", async () => {
    expect(OPENPANEL_INSIGHTS_LIMIT_MAX).toBe(100);
    await harness!.service.topPages("proj", { limit: 5000 });
    expect(new URL(harness!.captured[0]!.url).searchParams.get("limit")).toBe("100");
  });

  it("names the /insights/pages params the server silently ignores", () => {
    expect([...OPENPANEL_IGNORED_INSIGHTS_PARAMS]).toEqual(["cursor", "limit"]);
  });
});

describe("openpanel filters", () => {
  const filters: OpenPanelFilter[] = [
    { name: "path", operator: "contains", value: ["/checkout"] },
    { name: "revenue", operator: "gt", value: [0] },
  ];

  it("serialises to ONE URL-encoded JSON string param", async () => {
    await harness!.service.listExportEvents({ filters });
    const raw = new URL(harness!.captured[0]!.url).searchParams.get("filters")!;
    expect(JSON.parse(raw)).toEqual(filters);
    // One param, not one per filter.
    expect(new URL(harness!.captured[0]!.url).searchParams.getAll("filters")).toHaveLength(1);
  });

  it("percent-encodes the JSON so & and = inside values survive", () => {
    const filters: OpenPanelFilter[] = [{ name: "q", operator: "is", value: ["a&b=c"] }];
    const serialized = serializeOpenPanelFilters(filters)!;
    expect(serialized).toContain("a&b=c");

    // Encoding is NOT the caller's job: a raw concatenation truncates the JSON at
    // the first `&` and the API silently drops the whole filter. buildQuery is
    // what makes the round trip safe.
    const search = buildQuery({ filters: serialized });
    expect(search).not.toContain("&=");
    expect(JSON.parse(new URLSearchParams(search).get("filters")!)).toEqual(filters);
  });

  it("omits the param entirely when there are no filters", async () => {
    await harness!.service.listExportEvents();
    expect(new URL(harness!.captured[0]!.url).searchParams.has("filters")).toBe(false);
  });

  it("validates the operator, because bad filter JSON is silently dropped upstream", () => {
    expect(() =>
      serializeOpenPanelFilters([{ name: "x", operator: "soundsLike" as never, value: ["y"] }]),
    ).toThrow(OpenPanelValidationError);
    expect(() =>
      serializeOpenPanelFilters([{ name: "x", operator: "soundsLike" as never, value: ["y"] }]),
    ).toThrow(/silently/);
  });

  it("refuses a filter blob that would be truncated by maxParamLength", () => {
    const huge: OpenPanelFilter[] = Array.from({ length: 400 }, (_, i) => ({
      name: `field_${i}`,
      operator: "is" as const,
      value: ["x".repeat(60)],
    }));
    expect(() => serializeOpenPanelFilters(huge)).toThrow(/maxParamLength/);
  });
});

describe("openpanel time windows", () => {
  it("flags a bare `range` as resolved by the server in the PROJECT timezone", () => {
    expect(describeOpenPanelTimeWindow({ range: "last24h" })).toEqual({
      range: "last24h",
      resolvedByServerInProjectTimezone: true,
    });
  });

  it("does not flag an explicit startDate/endDate window", () => {
    expect(
      describeOpenPanelTimeWindow({
        range: "last24h",
        startDate: "2026-08-01",
        endDate: "2026-08-07",
      }).resolvedByServerInProjectTimezone,
    ).toBe(false);
  });

  it("sends range, startDate and endDate as separate params", async () => {
    await harness!.service.overview("proj", {
      range: "custom",
      startDate: "2026-08-01",
      endDate: "2026-08-07",
    });
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.get("range")).toBe("custom");
    expect(url.searchParams.get("startDate")).toBe("2026-08-01");
    expect(url.searchParams.get("endDate")).toBe("2026-08-07");
  });
});

describe("openpanel funnel and range validation", () => {
  it("rejects fewer than 2 steps and more than 10", async () => {
    await expect(harness!.service.funnel("proj", { steps: ["a"] })).rejects.toThrow(/2\.\.10/);
    await expect(
      harness!.service.funnel("proj", { steps: Array.from({ length: 11 }, (_, i) => `s${i}`) }),
    ).rejects.toThrow(/2\.\.10/);
  });

  it("rejects a windowHours outside 1..720 — a 30-day click is 720 and one hour over", async () => {
    await expect(
      harness!.service.funnel("proj", { steps: ["a", "b"], windowHours: 0 }),
    ).rejects.toThrow(/1\.\.720/);
    await expect(
      harness!.service.funnel("proj", { steps: ["a", "b"], windowHours: 721 }),
    ).rejects.toThrow(/1\.\.720/);
  });

  it("sends the steps in order, repeated by default", async () => {
    await harness!.service.funnel("proj", {
      steps: ["view", "buy"],
      windowHours: 48,
      groupBy: "profile_id",
    });
    const url = new URL(harness!.captured[0]!.url);
    // Repeated, not comma-joined: see OpenPanelStepEncoding for why the bad case
    // for this choice is a loud 400 rather than a silently-zero funnel.
    expect(url.searchParams.getAll("steps")).toEqual(["view", "buy"]);
    expect(url.searchParams.get("windowHours")).toBe("48");
    expect(url.searchParams.get("groupBy")).toBe("profile_id");
  });

  it("supports the csv step encoding for deployments that split on commas", async () => {
    const service = new OpenPanelAdapterService(new Context(), {
      baseUrl: "https://analytics.test",
      clientId: VALID_CLIENT_ID,
      clientSecret: "shh",
      funnelStepEncoding: "csv",
    });
    const before = harness!.captured.length;
    await service.funnel("proj", { steps: ["view", "buy"] });
    expect(new URL(harness!.captured[before]!.url).searchParams.get("steps")).toBe("view,buy");
  });

  it("rejects activeUsers days outside 1..90", async () => {
    await expect(harness!.service.activeUsers("proj", { days: 91 })).rejects.toThrow(/1\.\.90/);
  });
});

describe("openpanel unverified surfaces", () => {
  it("returns exportCharts and userFlow as unknown, not a guessed shape", async () => {
    const charts = await harness!.service.exportCharts({ projectId: "proj" });
    const flow = await harness!.service.userFlow("proj");
    // Typed `unknown`: nothing here claims a node/link schema that was never verified.
    expect(charts).toBeDefined();
    expect(flow).toBeDefined();
    // export/charts is always executed as linear/sum server-side.
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.get("chartType")).toBe("linear");
    expect(url.searchParams.get("metric")).toBe("sum");
  });

  it("models no replay surface at all", async () => {
    await harness!.service.overview("proj");
    for (const call of harness!.captured) {
      expect(call.url).not.toMatch(/replay/i);
    }
  });

  it("reads manage/projects as {data: [...]} with ISO createdAt", async () => {
    const projects = await harness!.service.manageProjects();
    expect(Array.isArray(projects.data)).toBe(true);
    expect(projects.data[0]!.createdAt).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });
});

describe("openpanel self-throttling", () => {
  it("records each request and waits once the window is full", async () => {
    let now = 0;
    const throttle = new SlidingWindowThrottle(3, 10_000, () => now);
    throttle.record();
    throttle.record();
    expect(throttle.capacity).toBe(3);
    // Not full yet: no wait.
    throttle.record();
    const stamps = (throttle as unknown as { stamps: number[] }).stamps;
    expect(stamps).toHaveLength(3);
    now = 20_000;
    throttle.record();
    expect((throttle as unknown as { stamps: number[] }).stamps).toHaveLength(1);
  });

  it("documents 100/10s for export and insights and 20/10s for manage", async () => {
    const { OPENPANEL_RATE_LIMITS } = await import("../src/service.js");
    expect(OPENPANEL_RATE_LIMITS.export).toEqual({ limit: 100, windowMs: 10_000 });
    expect(OPENPANEL_RATE_LIMITS.insights).toEqual({ limit: 100, windowMs: 10_000 });
    expect(OPENPANEL_RATE_LIMITS.manage).toEqual({ limit: 20, windowMs: 10_000 });
  });
});

describe("openpanel manifest", () => {
  it("declares the documented shape and the required client type", () => {
    expect(openPanelManifest.id).toBe("openpanel");
    expect(openPanelManifest.uiPath).toBe("/plugins/openpanel");
    expect(openPanelManifest.upstream).toEqual({ product: "OpenPanel", envPrefix: "OPENPANEL" });
    expect(openPanelManifest.agent!.skills.length).toBeGreaterThan(0);
    // The client-type gate is stated in the manifest, not just in a comment.
    expect(openPanelManifest.description).toMatch(/type/i);
  });
});
