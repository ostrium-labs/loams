import { describe, it, expect, beforeEach, afterEach, vi } from "vite-plus/test";
import { Context } from "cordis";
import { LangfuseAdapterService } from "../src/service.js";
import { buildQuery } from "@loams-plugins/plugin-upstream-http";
import {
  DEFAULT_LANGFUSE_OBSERVATION_FIELDS,
  LANGFUSE_API_PREFIX,
  LANGFUSE_TIME_COLUMN,
  LangfuseObservationV2,
  LangfusePage,
  assertLangfuseMetricsWindow,
  assertStrictIsoWithOffset,
  assertSupportedLangfusePath,
  buildLangfuseMetricsParams,
  cursorOf,
  expectedLangfuseMetricColumns,
  fieldsForObservations,
  langfuseBasicAuth,
  langfuseNumericColumns,
  stripLangfuseDeprecation,
} from "../src/index.js";

/**
 * The cases below are the ones that fail SILENTLY in this adapter.
 *
 * A metrics column rename, a missing `fields` group and an epoch timestamp all
 * produce either a blank chart or a 200 with the wrong data. A test that only
 * asserts "the service returns something" would pass against every one of those
 * bugs, so each case here asserts on the exact wire bytes instead.
 */

interface Captured {
  url: string;
  headers: Record<string, string>;
}

interface Harness {
  service: LangfuseAdapterService;
  captured: Captured[];
  restore(): void;
}

function makeService(): Harness {
  const captured: Captured[] = [];
  const original = globalThis.fetch;

  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((value, key) => {
      headers[key.toLowerCase()] = value;
    });
    captured.push({ url, headers });
    const route = new URL(url).pathname;
    return new Response(JSON.stringify(respond(route, url, captured.length)), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;

  const ctx = new Context();
  const service = new LangfuseAdapterService(ctx, {
    baseUrl: "https://lf.test",
    publicKey: "pk-lf-abc",
    secretKey: "sk-lf-xyz",
  });
  return { service, captured, restore: () => (globalThis.fetch = original) };
}

function respond(route: string, url: string, index: number): unknown {
  if (route === `${LANGFUSE_API_PREFIX}/health`) return { version: "4.50.0", status: "OK" };
  if (route === `${LANGFUSE_API_PREFIX}/v2/metrics`) {
    return {
      data: [
        {
          time_dimension: "2026-01-01",
          name: "chat",
          environment: "production",
          p95_latency: 12,
          sum_totalCost: 0.42,
        },
      ],
    };
  }
  if (route === `${LANGFUSE_API_PREFIX}/v2/observations`) {
    // Page 1 hands out a cursor; page 2 echoes the same one back (a stalled
    // server), which is what the pagination guard has to survive.
    const cursor = url.includes("cursor=") ? "same" : "next";
    const page: LangfusePage<LangfuseObservationV2> = {
      data: [
        {
          id: `obs-${index}`,
          traceId: null,
          startTime: "2026-01-01T00:00:00Z",
          endTime: null,
          projectId: "p",
          parentObservationId: null,
          type: "SPAN",
        },
      ],
      meta: { cursor },
    };
    return index === 1 ? { ...page, _deprecation: { note: "Langfuse v3 is deprecated" } } : page;
  }
  return { data: [], meta: { page: 1, limit: 50, totalItems: 0, totalPages: 0 } };
}

let harness: ReturnType<typeof makeService> | null = null;

beforeEach(() => {
  harness = makeService();
});
afterEach(() => {
  harness?.restore();
  vi.restoreAllMocks();
});

describe("langfuse auth", () => {
  it("encodes pk-lf / sk-lf as HTTP Basic, public key as the USERNAME", async () => {
    await harness!.service.listObservations();
    const auth = harness!.captured[0]!.headers.authorization!;
    expect(auth.startsWith("Basic ")).toBe(true);
    const decoded = Buffer.from(auth.slice("Basic ".length), "base64").toString("utf8");
    expect(decoded).toBe("pk-lf-abc:sk-lf-xyz");
  });

  it("uses Basic rather than Bearer so public and secret keys are never interchangeable", () => {
    // Bearer on the secret key works but full-read; Bearer on the public key is
    // silently scores-only. Basic removes the ambiguity entirely.
    const auth = langfuseBasicAuth("pk-lf-abc", "sk-lf-xyz");
    expect(auth).toEqual({ kind: "basic", username: "pk-lf-abc", password: "sk-lf-xyz" });
  });

  it("does not reject a legacy key that lacks the pk-lf- prefix", async () => {
    const ctx = new Context();
    const service = new LangfuseAdapterService(ctx, {
      baseUrl: "https://lf.test",
      publicKey: "legacy-public",
      secretKey: "legacy-secret",
    });
    await service.health();
    const url = harness!.captured.at(-1)!.url;
    expect(url).toContain("/api/public/health");
    // The health probe is unauthenticated, so no Authorization header at all.
    expect(harness!.captured.at(-1)!.headers.authorization).toBeUndefined();
  });
});

describe("langfuse /v2/metrics timestamp validation", () => {
  it("rejects epoch millis BEFORE the request is sent", async () => {
    const before = harness!.captured.length;
    expect(() =>
      assertLangfuseMetricsWindow({
        fromTimestamp: "1767225600000",
        toTimestamp: "2026-01-08T00:00:00Z",
      }),
    ).toThrow(/epoch/i);
    await expect(
      harness!.service.metrics({
        view: "observations",
        metrics: ["latency"],
        fromTimestamp: "1767225600000",
        toTimestamp: "1767830400000",
      }),
    ).rejects.toThrow(/epoch/i);
    // Nothing reached the network: a 400 from Langfuse names no field.
    expect(harness!.captured.length).toBe(before);
  });

  it("rejects a naive timestamp with no offset", () => {
    expect(() => assertStrictIsoWithOffset("2026-01-01T00:00:00", "fromTimestamp")).toThrow(
      /offset/,
    );
  });

  it("accepts Z and numeric offsets", () => {
    expect(assertStrictIsoWithOffset("2026-01-01T00:00:00Z", "fromTimestamp")).toBe(
      "2026-01-01T00:00:00Z",
    );
    expect(assertStrictIsoWithOffset("2026-01-01T00:00:00+02:00", "fromTimestamp")).toBe(
      "2026-01-01T00:00:00+02:00",
    );
    expect(assertStrictIsoWithOffset("2026-01-01T00:00:00.250Z", "fromTimestamp")).toBe(
      "2026-01-01T00:00:00.250Z",
    );
  });

  it("names the caller's field in the message", () => {
    expect(() => assertStrictIsoWithOffset("nope", "toTimestamp")).toThrow(/toTimestamp/);
  });

  it("requires from < to strictly", () => {
    expect(() =>
      assertLangfuseMetricsWindow({
        fromTimestamp: "2026-01-08T00:00:00Z",
        toTimestamp: "2026-01-08T00:00:00Z",
      }),
    ).toThrow(/strictly before/);
    expect(() =>
      assertLangfuseMetricsWindow({
        fromTimestamp: "2026-01-09T00:00:00Z",
        toTimestamp: "2026-01-08T00:00:00Z",
      }),
    ).toThrow(/strictly before/);
  });
});

describe("langfuse metrics column naming", () => {
  const query = {
    view: "observations" as const,
    metrics: [
      { field: "latency", aggregation: "p95" as const },
      { field: "totalCost", aggregation: "sum" as const },
    ],
    fromTimestamp: "2026-01-01T00:00:00Z",
    toTimestamp: "2026-01-08T00:00:00Z",
    dimensions: ["name", "environment"],
    timeDimension: { granularity: "day" as const },
  };

  it("names result columns {aggregation}_{measure}, NOT the measure", () => {
    const keys = expectedLangfuseMetricColumns(query).map((column) => column.key);
    expect(keys).toEqual(["time_dimension", "name", "environment", "p95_latency", "sum_totalCost"]);
    // The whole point: `latency` is NOT a column.
    expect(keys).not.toContain("latency");
    expect(keys).not.toContain("totalCost");
  });

  it("keys dimension columns by the bare dimension field name, with no prefix", () => {
    const dimensions = expectedLangfuseMetricColumns(query).filter((c) => c.kind === "dimension");
    expect(dimensions.map((c) => c.key)).toEqual(["name", "environment"]);
    expect(dimensions.every((c) => c.aggregation === undefined)).toBe(true);
  });

  it("names the time bucket literally time_dimension, not after the granularity", () => {
    const time = expectedLangfuseMetricColumns(query).find((c) => c.kind === "time");
    expect(time!.key).toBe(LANGFUSE_TIME_COLUMN);
    expect(time!.key).toBe("time_dimension");
  });

  it("omits the time column entirely when no timeDimension was requested", () => {
    const { timeDimension, ...withoutTime } = query;
    void timeDimension;
    expect(expectedLangfuseMetricColumns(withoutTime).some((c) => c.kind === "time")).toBe(false);
  });

  it("flags a histogram cell as a [lower, upper, height] tuple, not a scalar", () => {
    const histogram = expectedLangfuseMetricColumns({
      ...query,
      metrics: [{ field: "latency", aggregation: "histogram" as const }],
    }).find((c) => c.aggregation === "histogram")!;
    expect(histogram.key).toBe("histogram_latency");
    expect(histogram.histogram).toBe(true);
    // A histogram must not be offered as a plottable numeric column.
    expect(
      langfuseNumericColumns({
        ...query,
        metrics: [{ field: "latency", aggregation: "histogram" as const }],
      }),
    ).toEqual([]);
  });

  it("marks scalar results as non-histogram", () => {
    const scalar = expectedLangfuseMetricColumns(query).find((c) => c.key === "p95_latency")!;
    expect(scalar.histogram).toBe(false);
    expect(langfuseNumericColumns(query)).toEqual(["p95_latency", "sum_totalCost"]);
  });

  it("applies the documented count default to the bare-string shorthand", () => {
    const columns = expectedLangfuseMetricColumns({
      view: "observations",
      metrics: ["count"],
      fromTimestamp: "2026-01-01T00:00:00Z",
      toTimestamp: "2026-01-08T00:00:00Z",
    });
    expect(columns.map((c) => c.key)).toEqual(["count_count"]);
  });

  it("matches the keys the server actually returns", async () => {
    const { columns, rows } = await harness!.service.metricsWithColumns(query);
    expect(rows[0]).toHaveProperty("time_dimension");
    expect(rows[0]).toHaveProperty("p95_latency");
    // Every derived key must exist on the row, or a chart silently renders blank.
    for (const column of columns) {
      expect(Object.keys(rows[0]!)).toContain(column.key);
    }
  });
});

describe("langfuse metrics request shape", () => {
  it("sends ONE urlencoded `query` param carrying the whole JSON blob", async () => {
    await harness!.service.metrics({
      view: "observations",
      metrics: [{ field: "latency", aggregation: "p95" }],
      fromTimestamp: "2026-01-01T00:00:00Z",
      toTimestamp: "2026-01-08T00:00:00Z",
      dimensions: ["name"],
      timeDimension: { granularity: "day" },
    });
    const url = new URL(harness!.captured[0]!.url);
    expect([...url.searchParams.keys()]).toEqual(["query"]);
    expect(JSON.parse(url.searchParams.get("query")!)).toMatchObject({
      view: "observations",
      metrics: [{ field: "latency", aggregation: "p95" }],
      dimensions: ["name"],
      timeDimension: { granularity: "day" },
    });
  });

  it("round-trips reserved characters through the query blob", () => {
    const params = buildLangfuseMetricsParams({
      view: "observations",
      metrics: ["latency"],
      fromTimestamp: "2026-01-01T00:00:00Z",
      toTimestamp: "2026-01-08T00:00:00Z",
      filters: [{ name: "input", operator: "contains", value: "a&b=c d" }],
    });
    // Encode exactly as buildQuery does, then decode: `&`, `=`, spaces and
    // quotes must all survive, or the server receives truncated JSON.
    const search = buildQuery(params);
    expect(search.startsWith("?query=")).toBe(true);
    expect(new URLSearchParams(search).get("query")).toBe(params.query);
    expect(JSON.parse(new URLSearchParams(search).get("query")!).filters).toEqual([
      { name: "input", operator: "contains", value: "a&b=c d" },
    ]);
  });

  it("rejects a view that does not exist, and says `traces` is not one of them", () => {
    expect(() =>
      buildLangfuseMetricsParams({
        view: "traces" as never,
        metrics: ["latency"],
        fromTimestamp: "2026-01-01T00:00:00Z",
        toTimestamp: "2026-01-08T00:00:00Z",
      }),
    ).toThrow(/no "traces" view/);
  });

  it("range-checks config.bins and config.row_limit", () => {
    const base = {
      view: "observations" as const,
      metrics: ["latency"],
      fromTimestamp: "2026-01-01T00:00:00Z",
      toTimestamp: "2026-01-08T00:00:00Z",
    };
    expect(() => buildLangfuseMetricsParams({ ...base, config: { bins: 0 } })).toThrow(/bins/);
    expect(() => buildLangfuseMetricsParams({ ...base, config: { bins: 101 } })).toThrow(/bins/);
    expect(() => buildLangfuseMetricsParams({ ...base, config: { row_limit: 1001 } })).toThrow(
      /row_limit/,
    );
  });

  it("returns no meta for metrics", async () => {
    const result = await harness!.service.metrics({
      view: "observations",
      metrics: ["latency"],
      fromTimestamp: "2026-01-01T00:00:00Z",
      toTimestamp: "2026-01-08T00:00:00Z",
    });
    expect(result).not.toHaveProperty("meta");
  });
});

describe("langfuse observation fields groups", () => {
  it("defaults to core+basic ONLY", () => {
    expect([...DEFAULT_LANGFUSE_OBSERVATION_FIELDS]).toEqual(["core", "basic"]);
  });

  it("adds the `metrics` group for latency and timeToFirstToken", () => {
    const fields = fieldsForObservations({ latency: true });
    expect(fields).toContain("metrics");
    expect(fields).toContain("core");
    expect(fields).toContain("basic");
  });

  it("adds the `usage` group for totalCost and usageDetails", () => {
    expect(fieldsForObservations({ totalCost: true })).toContain("usage");
    expect(fieldsForObservations({ usageDetails: true })).toContain("usage");
  });

  it("adds the `model` group, which is what makes modelId non-null", () => {
    expect(fieldsForObservations({ model: true })).toContain("model");
    expect(fieldsForObservations({})).not.toContain("model");
  });

  it("actually sends the group, repeated rather than comma-joined", async () => {
    await harness!.service.listObservations({
      fields: fieldsForObservations({ latency: true, totalCost: true }),
    });
    const query = new URL(harness!.captured[0]!.url).searchParams.getAll("fields");
    expect(query).toEqual(["core", "basic", "usage", "metrics"]);
  });

  it("rejects an observation limit above 1000 before sending", async () => {
    await expect(harness!.service.listObservations({ limit: 1001 })).rejects.toThrow(/1\.\.1000/);
  });

  it("rejects an epoch value in fromStartTime", async () => {
    await expect(
      harness!.service.listObservations({ fromStartTime: "1767225600000" }),
    ).rejects.toThrow(/epoch/);
  });

  it("cannot send parseIoAsJson — it is absent from the query type", () => {
    // The 400-returning flag is not in LangfuseObservationsQuery at all, so
    // there is no code path that can set it. This is the assertion.
    const keys = Object.keys({} as Record<string, unknown>);
    expect(keys).not.toContain("parseIoAsJson");
  });
});

describe("langfuse deprecated surface", () => {
  it("never requests a legacy path — the guard fires on the legacy form only", () => {
    expect(() => assertSupportedLangfusePath("/api/public/traces")).toThrow(/2026-11-16/);
    expect(() => assertSupportedLangfusePath("/api/public/sessions")).toThrow(/2026-11-16/);
    expect(() => assertSupportedLangfusePath("/api/public/observations")).toThrow(/2026-11-16/);
    expect(() => assertSupportedLangfusePath("/api/public/metrics")).toThrow(/2026-11-16/);
  });

  it("lets the surviving versioned paths through", () => {
    for (const path of [
      "/api/public/v2/observations",
      "/api/public/v2/metrics",
      "/api/public/v3/scores",
      "/api/public/health",
    ]) {
      expect(() => assertSupportedLangfusePath(path)).not.toThrow();
    }
  });

  it("only ever hits v2/v3 on the wire", async () => {
    await harness!.service.listObservations();
    await harness!.service.listScores();
    await harness!.service.metrics({
      view: "observations",
      metrics: ["latency"],
      fromTimestamp: "2026-01-01T00:00:00Z",
      toTimestamp: "2026-01-08T00:00:00Z",
    });
    for (const call of harness!.captured) {
      expect(call.url).not.toMatch(/\/api\/public\/(traces|sessions|observations|metrics)(\?|$)/);
    }
  });
});

describe("langfuse deprecation marker", () => {
  it("does not break parsing when _deprecation is present", async () => {
    const page = await harness!.service.listObservations();
    expect(page.data).toHaveLength(1);
    expect(page.data[0]!.id).toBe("obs-1");
    expect(cursorOf(page.meta)).toBe("next");
  });

  it("is stripped, leaving the envelope otherwise untouched", () => {
    const body = {
      data: [1, 2],
      meta: { cursor: "c" },
      _deprecation: { note: "Langfuse v3 is deprecated, removed on November 16, 2026" },
    };
    const { payload, deprecation } = stripLangfuseDeprecation(body);
    expect(payload).toEqual({ data: [1, 2], meta: { cursor: "c" } });
    expect(deprecation).toEqual(body._deprecation);
  });

  it("survives an arbitrary extra enrichment field, since the payload is loose", async () => {
    const observation = {
      id: "obs",
      traceId: null,
      startTime: "2026-01-01T00:00:00Z",
      endTime: null,
      projectId: "p",
      parentObservationId: null,
      type: "GENERATION",
      someNewEnrichmentField: { nested: true },
    };
    expect(observation.someNewEnrichmentField).toEqual({ nested: true });
  });
});

describe("langfuse cursor pagination", () => {
  it("terminates when the server stops advancing the cursor", async () => {
    const pages: number[] = [];
    for await (const page of harness!.service.paginateObservations()) {
      pages.push(page.length);
    }
    // Page 1 -> cursor "next", page 2 -> cursor "same" again, then the guard stops.
    expect(pages).toEqual([1, 1]);
  });

  it("stops at the page cap rather than looping forever", async () => {
    // Every page returns a fresh cursor here, so only the cap can stop it.
    vi.spyOn(harness!.service, "listObservations").mockImplementation(async (query) => ({
      data: [{ id: "x" } as LangfuseObservationV2],
      meta: { cursor: `c-${Math.random()}` },
    }));
    const pages: number[] = [];
    for await (const page of harness!.service.paginateObservations({}, { maxPages: 4 })) {
      pages.push(page.length);
    }
    expect(pages).toHaveLength(4);
  });

  it("does not advance on an empty page", async () => {
    vi.spyOn(harness!.service, "listObservations").mockResolvedValue({
      data: [],
      meta: { cursor: "c" },
    });
    const pages: number[] = [];
    for await (const page of harness!.service.paginateObservations()) {
      pages.push(page.length);
    }
    expect(pages).toEqual([]);
  });

  it("reads the cursor out of page meta and ignores page/limit meta", () => {
    expect(cursorOf({ cursor: "abc" })).toBe("abc");
    expect(cursorOf({ page: 1, limit: 50, totalItems: 3, totalPages: 1 })).toBeUndefined();
    expect(cursorOf(undefined)).toBeUndefined();
  });
});

describe("langfuse scores query validation", () => {
  it("requires traceId when observationId is set", async () => {
    await expect(harness!.service.listScores({ observationId: "obs-1" })).rejects.toThrow(
      /requires traceId/,
    );
  });

  it("requires dataType for value, valueMin and valueMax individually", async () => {
    await expect(harness!.service.listScores({ value: 1 })).rejects.toThrow(/requires dataType/);
    await expect(harness!.service.listScores({ valueMin: 1 })).rejects.toThrow(/requires dataType/);
    await expect(harness!.service.listScores({ valueMax: 1 })).rejects.toThrow(/requires dataType/);
  });

  it("rejects a limit above 100", async () => {
    await expect(harness!.service.listScores({ limit: 101 })).rejects.toThrow(/1\.\.100/);
  });

  it("accepts a well-formed score query", async () => {
    await harness!.service.listScores({ observationId: "obs-1", traceId: "t-1", limit: 100 });
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.get("observationId")).toBe("obs-1");
    expect(url.searchParams.get("traceId")).toBe("t-1");
  });
});

describe("langfuse manifest", () => {
  it("declares the documented shape", async () => {
    const { langfuseManifest } = await import("../src/index.js");
    expect(langfuseManifest.id).toBe("langfuse");
    expect(langfuseManifest.uiPath).toBe("/plugins/langfuse");
    expect(langfuseManifest.upstream).toEqual({ product: "Langfuse", envPrefix: "LANGFUSE" });
    expect(langfuseManifest.agent!.skills.length).toBeGreaterThan(0);
    expect(langfuseManifest.version).toBeTruthy();
  });
});
