import { describe, it, expect, vi, beforeEach, afterEach } from "vite-plus/test";
import { Context } from "cordis";
import {
  MATOMO_METHOD_NAMES,
  MatomoAdapterService,
  MatomoApiError,
  classifyMatomoResponse,
  matomoLoader,
  matomoManifest,
  matomoNumber,
  matomoPercent,
  normaliseMatomoResponse,
} from "../src/service.js";
import { MATOMO_METHOD_NAME_PATTERN, MATOMO_ENDPOINT } from "../src/types.js";
import type { MatomoConfig, MatomoVisitsSummaryRow } from "../src/types.js";
import type { PluginRuntime } from "@loams-plugins/core";

type MockedFetch = ReturnType<typeof vi.fn<typeof fetch>>;

const BASE = "https://matomo.example.test";

/** `PluginLoader.skills` receives a runtime its handlers never read. */
const RUNTIME_STUB = {} as PluginRuntime;

/**
 * A JSON response. Always stringified — passing a JS string for a bare-scalar
 * body must still produce valid JSON, or the shared client reports a
 * "non-JSON body" upstream error instead of the shape under test.
 */
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

/** The URL of the nth (0-based) fetch call. */
function urlOf(mock: MockedFetch, index = 0): URL {
  return new URL(urlString(mock.mock.calls[index]?.[0]));
}

/** All decoded query params of the nth call, lower-cased keys preserved as sent. */
function paramsOf(mock: MockedFetch, index = 0): URLSearchParams {
  return urlOf(mock, index).searchParams;
}

/** The Authorization header of the nth call. */
function authHeaderOf(mock: MockedFetch, index = 0): string | null {
  const init = mock.mock.calls[index]?.[1] as RequestInit | undefined;
  return new Headers(init?.headers).get("Authorization");
}

describe("MatomoAdapterService", () => {
  let service: MatomoAdapterService;
  let fetchMock: MockedFetch;

  function makeService(overrides: Partial<MatomoConfig> = {}): MatomoAdapterService {
    return new MatomoAdapterService(new Context(), {
      baseUrl: BASE,
      apiToken: "TOKENAUTHVALUE",
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
  /* Request shape                                                       */
  /* ------------------------------------------------------------------ */

  it("targets /index.php — 5.14.0 has no /api/v1 REST prefix", async () => {
    fetchMock.mockResolvedValueOnce(reply({ nb_visits: 5 }));

    await service.getVisitsSummary({ idSite: 1, date: "2026-09-01" });

    expect(urlOf(fetchMock).pathname).toBe(MATOMO_ENDPOINT);
    expect(urlOf(fetchMock).pathname).toBe("/index.php");
    expect(urlString(fetchMock.mock.calls[0]?.[0])).not.toContain("/api/v1");
  });

  it("sends module=API, a dot-form method, format=JSON and idSite camelCase", async () => {
    fetchMock.mockResolvedValueOnce(reply({ nb_visits: 5 }));

    await service.getVisitsSummary({ idSite: 1, period: "day", date: "2026-09-01" });

    const params = paramsOf(fetchMock);
    expect(params.get("module")).toBe("API");
    expect(params.get("method")).toBe("VisitsSummary.get");
    expect(params.get("format")).toBe("JSON");
    expect(params.get("idSite")).toBe("1");
  });

  it("never sends the lowercase `idsite` on any method", async () => {
    fetchMock.mockImplementation(async () => reply({ nb_visits: 1 }));

    await service.getVisitsSummary({ idSite: 1 });
    await service.getPageUrls({ idSite: 1 });
    await service.getCountries({ idSite: "1,4" });
    await service.getDeviceTypes({ idSite: "all" });

    expect(fetchMock.mock.calls.length).toBe(4);
    for (const call of fetchMock.mock.calls) {
      const params = new URL(urlString(call[0])).searchParams;
      expect(params.has("idsite")).toBe(false);
      expect(params.has("idSite")).toBe(true);
    }
  });

  it("refuses to build a request carrying a lowercase `idsite` parameter", () => {
    // The public API takes `idSite`; the guard is what stops an internal caller
    // (or a future option) from reintroducing the lowercase spelling, which the
    // server rejects with General_PleaseSpecifyValue.
    const internals = service as unknown as {
      _buildParams: (m: string, p?: Record<string, unknown>) => unknown;
    };
    expect(() => internals._buildParams("VisitsSummary.get", { idsite: 1 })).toThrow(
      /`idSite` with a capital S and it is CASE-SENSITIVE/,
    );
    expect(fetchMock.mock.calls.length).toBe(0);
  });

  it("uses DevicesDetection and UserCountry as the module, never Devices or UserCountries", async () => {
    fetchMock.mockImplementation(async () => reply([]));

    await service.getDeviceTypes({ idSite: 1 });
    await service.getCountries({ idSite: 1 });
    await service.getBrowsers({ idSite: 1 });
    await service.getContinents({ idSite: 1 });

    const modules = fetchMock.mock.calls.map(
      (call) => new URL(urlString(call[0])).searchParams.get("method") ?? "",
    );
    expect(modules).toEqual([
      "DevicesDetection.getType",
      "UserCountry.getCountry",
      "DevicesDetection.getBrowsers",
      "UserCountry.getContinent",
    ]);
    for (const method of modules) {
      expect(method.startsWith("Devices.")).toBe(false);
      expect(method.startsWith("UserCountries")).toBe(false);
    }
  });

  it("calls VisitTime.getByDayOfWeek, not a getDayOfWeek method", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.getVisitsByDayOfWeek({ idSite: 1, period: "week", date: "last4" });

    expect(paramsOf(fetchMock).get("method")).toBe("VisitTime.getByDayOfWeek");
  });

  it("never references the non-existent VisitsSummary.getEvolution", () => {
    expect(MATOMO_METHOD_NAMES).not.toContain("VisitsSummary.getEvolution" as never);
    for (const method of MATOMO_METHOD_NAMES) {
      expect(method).not.toContain("getEvolution");
      expect(method).not.toMatch(/getDayOfWeek$/);
    }
  });

  /* ------------------------------------------------------------------ */
  /* Method-name grammar                                                 */
  /* ------------------------------------------------------------------ */

  it("every declared method name is exactly two dot-separated parts", () => {
    for (const method of MATOMO_METHOD_NAMES) {
      expect(method).toMatch(MATOMO_METHOD_NAME_PATTERN);
      expect(method.split(".")).toHaveLength(2);
      expect(method).not.toContain("/");
    }
  });

  it("refuses a slash-form method name before it reaches the network", async () => {
    const broken = service as unknown as {
      _buildParams: (m: string, p?: Record<string, unknown>) => unknown;
    };
    expect(() => broken._buildParams("VisitsSummary/get")).toThrow(
      /Expected exactly 'Module\.methodName'/,
    );
    expect(fetchMock.mock.calls.length).toBe(0);
  });

  /* ------------------------------------------------------------------ */
  /* format_metrics and filter_limit                                    */
  /* ------------------------------------------------------------------ */

  /**
   * A per-method reply, so one test can exercise several endpoints that answer
   * with DIFFERENT shapes — which is the point of the union.
   */
  function replyByMethod(bodies: Record<string, unknown>): void {
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const method = new URL(urlString(input)).searchParams.get("method") ?? "";
      const body = Object.prototype.hasOwnProperty.call(bodies, method) ? bodies[method] : {};
      return reply(body);
    });
  }

  it("always sends format_metrics=0", async () => {
    replyByMethod({
      "VisitsSummary.get": { nb_visits: 1 },
      "Actions.get": { nb_visits: 1 },
      "Actions.getPageUrls": [],
      "Referrers.getWebsites": [],
      "Goals.get": [],
      "Live.getLastVisitsDetails": [],
      "SitesManager.getAllSitesId": [1, 4],
    });

    await service.getVisitsSummary({ idSite: 1 });
    await service.getActions({ idSite: 1 });
    await service.getPageUrls({ idSite: 1 });
    await service.getReferrerWebsites({ idSite: 1 });
    await service.getGoals({ idSite: 1 });
    await service.getLastVisitsDetails({ idSite: 1 });
    await service.listSiteIds();

    expect(fetchMock.mock.calls.length).toBe(7);
    for (const call of fetchMock.mock.calls) {
      expect(new URL(urlString(call[0])).searchParams.get("format_metrics")).toBe("0");
    }
  });

  it("always sends an explicit filter_limit, defaulting to API_datatable_default_limit", async () => {
    fetchMock.mockImplementation(async () => reply({ nb_visits: 1 }));

    await service.getVisitsSummary({ idSite: 1 });
    expect(paramsOf(fetchMock).get("filter_limit")).toBe("100");

    await service.getPageUrls({ idSite: 1, rowLimit: 25 });
    expect(paramsOf(fetchMock, 1).get("filter_limit")).toBe("25");

    await service.getPageUrls({ idSite: 1, rowLimit: -1 });
    expect(paramsOf(fetchMock, 2).get("filter_limit")).toBe("-1");
  });

  it("honours a configured default row limit", async () => {
    fetchMock.mockResolvedValueOnce(reply({ nb_visits: 1 }));
    const custom = makeService({ defaultRowLimit: 5 });

    await custom.getVisitsSummary({ idSite: 1 });
    expect(paramsOf(fetchMock).get("filter_limit")).toBe("5");
  });

  it("passes filter_limit and filter_offset explicitly to Live.getLastVisitsDetails", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.getLastVisitsDetails({ idSite: 1, rowLimit: 30, offset: 60 });

    const params = paramsOf(fetchMock);
    expect(params.get("method")).toBe("Live.getLastVisitsDetails");
    expect(params.get("filter_limit")).toBe("30");
    expect(params.get("filter_offset")).toBe("60");
  });

  /* ------------------------------------------------------------------ */
  /* Auth                                                               */
  /* ------------------------------------------------------------------ */

  it("sends the token_auth as an Authorization: Bearer header", async () => {
    fetchMock.mockResolvedValueOnce(reply({ nb_visits: 1 }));

    await service.getVisitsSummary({ idSite: 1 });

    expect(authHeaderOf(fetchMock)).toBe("Bearer TOKENAUTHVALUE");
  });

  it("never puts token_auth in the query string, so the two sources cannot conflict", async () => {
    replyByMethod({
      "VisitsSummary.get": { nb_visits: 1 },
      "Actions.getPageTitles": [],
      "API.getMatomoVersion": "5.14.0",
    });

    await service.getVisitsSummary({ idSite: 1 });
    await service.getPageTitles({ idSite: 1 });
    await service.getVersion();

    for (const call of fetchMock.mock.calls) {
      const url = urlString(call[0]);
      expect(url).not.toContain("token_auth");
      expect(url).not.toContain("api_key");
      expect(new URL(url).searchParams.has("token_auth")).toBe(false);
    }
  });

  it("refuses to send token_auth as a query parameter next to the Bearer header", async () => {
    const internals = service as unknown as {
      _buildParams: (m: string, p?: Record<string, unknown>) => unknown;
    };
    expect(() => internals._buildParams("VisitsSummary.get", { token_auth: "OTHER" })).toThrow(
      /throwIfValuesConflict/,
    );
  });

  it("refuses api_key, which does not exist in 5.14.0", async () => {
    const internals = service as unknown as {
      _buildParams: (m: string, p?: Record<string, unknown>) => unknown;
    };
    expect(() => internals._buildParams("VisitsSummary.get", { api_key: "x" })).toThrow(
      /does not exist in Matomo 5\.14\.0/,
    );
  });

  it("refuses an empty token at construction with the manual-provisioning message", () => {
    expect(() => makeService({ apiToken: "" })).toThrow(/NO create-token API method/);
  });

  /* ------------------------------------------------------------------ */
  /* Errors: branch on the body, not the status                         */
  /* ------------------------------------------------------------------ */

  it("raises on result=error even though the HTTP status is 200", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({ result: "error", message: "The token_auth is invalid.", backtrace: "…" }, 200),
    );

    const error = await service.getVisitsSummary({ idSite: 1 }).catch((err: unknown) => err);

    expect(error).toBeInstanceOf(MatomoApiError);
    expect((error as MatomoApiError).status).toBe(200);
    expect((error as MatomoApiError).serverMessage).toBe("The token_auth is invalid.");
    expect((error as Error).message).toMatch(/The token_auth is invalid/);
  });

  it("never turns an error body into empty chart data", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({ result: "error", message: "You can't access this resource" }),
    );

    await expect(service.getPageUrls({ idSite: 1 })).rejects.toBeInstanceOf(MatomoApiError);
  });

  it("still raises when result=error has no message", async () => {
    fetchMock.mockResolvedValueOnce(reply({ result: "error" }));

    await expect(service.getVisitsSummary({ idSite: 1 })).rejects.toBeInstanceOf(MatomoApiError);
  });

  it("extracts the message from an error body carried by a real 4xx", async () => {
    fetchMock.mockResolvedValueOnce(reply({ result: "error", message: "Invalid idSite" }, 400));

    const error = await service.getVisitsSummary({ idSite: 99 }).catch((err: unknown) => err);

    expect(error).toBeInstanceOf(MatomoApiError);
    expect((error as MatomoApiError).status).toBe(400);
    expect((error as Error).message).toMatch(/Invalid idSite/);
  });

  it("propagates a non-JSON upstream failure as an UpstreamError", async () => {
    fetchMock.mockResolvedValueOnce(new Response("<html>502</html>", { status: 502 }));

    const error = await service.getVisitsSummary({ idSite: 1 }).catch((err: unknown) => err);

    expect((error as Error).name).toBe("UpstreamError");
  });

  /* ------------------------------------------------------------------ */
  /* Union normalisation: one method, two response shapes                */
  /* ------------------------------------------------------------------ */

  it("normalises the SAME method's single-date flat object and its date-keyed range", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        nb_visits: 12,
        nb_actions: 30,
        bounce_count: 4,
        bounce_rate: 0.33,
        sum_visit_length: 900,
      }),
    );
    const single = await service.getVisitsSummary({ idSite: 1, date: "2026-09-01" });

    fetchMock.mockResolvedValueOnce(
      reply({
        "2026-09-01": { nb_visits: 12, nb_actions: 30, bounce_count: 4 },
        "2026-09-02": { nb_visits: 8, nb_actions: 20, bounce_count: 2 },
      }),
    );
    const range = await service.getVisitsSummary({
      idSite: 1,
      period: "range",
      date: "2026-09-01,2026-09-02",
    });

    // Same type, same shape, one row vs two.
    expect(single.rows).toHaveLength(1);
    expect(single.dates).toBeUndefined();
    expect(single.rows[0]?.nb_visits).toBe(12);

    expect(range.rows).toHaveLength(2);
    expect(range.dates).toEqual(["2026-09-01", "2026-09-02"]);
    expect(range.rows.map((row) => row.date)).toEqual(["2026-09-01", "2026-09-02"]);
    expect(range.rows[0]?.nb_visits).toBe(12);
    expect(range.rows[1]?.nb_visits).toBe(8);
  });

  it("orders date keys ascending even when the server returns them out of order", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        "2026-09-03": { nb_visits: 3 },
        "2026-09-01": { nb_visits: 1 },
        "2026-09-02": { nb_visits: 2 },
      }),
    );

    const result = await service.getVisitsSummary({
      idSite: 1,
      period: "range",
      date: "2026-09-01,2026-09-03",
    });

    expect(result.dates).toEqual(["2026-09-01", "2026-09-02", "2026-09-03"]);
    expect(result.rows.map((row) => row.nb_visits)).toEqual([1, 2, 3]);
  });

  it("normalises a multi-site SINGLE-date map as one row per site, with no dates", async () => {
    // Regression: the site arm of the union holds FLAT ROWS here, so iterating the
    // site value as a date map turns `nb_visits` into a fake "date" bucket.
    fetchMock.mockResolvedValueOnce(
      reply({
        "Example Site": { nb_visits: 3, nb_actions: 9 },
        "Second Site": { nb_visits: 4, nb_actions: 8 },
      }),
    );

    const result = await service.getVisitsSummary({ idSite: "1,4" });

    expect(result.rows).toEqual([
      { nb_visits: 3, nb_actions: 9, siteId: "Example Site" },
      { nb_visits: 4, nb_actions: 8, siteId: "Second Site" },
    ]);
    expect(result.dates).toBeUndefined();
    expect(result.sites).toEqual(["Example Site", "Second Site"]);
  });

  it("normalises a multi-site multi-date map with BOTH axes on each row", async () => {
    // `core/Archive/DataTable.php:414-421`: "The site ID index is always first",
    // so this nests two levels deep.
    fetchMock.mockResolvedValueOnce(
      reply({
        "Example Site": { "2026-09-01": { nb_visits: 1 }, "2026-09-02": { nb_visits: 2 } },
        "Second Site": { "2026-09-01": { nb_visits: 7 } },
      }),
    );

    const result = await service.getVisitsSummary({
      idSite: "1,4",
      period: "range",
      date: "2026-09-01,2026-09-02",
    });

    expect(result.rows.map((row) => [row.siteId, row.date, row.nb_visits])).toEqual([
      ["Example Site", "2026-09-01", 1],
      ["Example Site", "2026-09-02", 2],
      ["Second Site", "2026-09-01", 7],
    ]);
    // The date axis is per-site, so no global `dates` list is invented.
    expect(result.dates).toBeUndefined();
    expect(result.sites).toEqual(["Example Site", "Second Site"]);
  });

  it("treats an empty dimension report as zero rows, not as a scalar", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    const result = await service.getPageUrls({ idSite: 1 });

    expect(result.rows).toEqual([]);
  });

  /* ------------------------------------------------------------------ */
  /* Methods that reject a range on the server                          */
  /* ------------------------------------------------------------------ */

  it("Referrers.getAll refuses a multi-date request before spending a round trip", async () => {
    await expect(
      service.getReferrers({ idSite: 1, period: "range", date: "2026-09-01,2026-09-30" }),
    ).rejects.toBeInstanceOf(MatomoApiError);
    expect(fetchMock.mock.calls.length).toBe(0);
  });

  it("Referrers.getAll refuses idSite=all and a comma list", async () => {
    await expect(service.getReferrers({ idSite: "all" })).rejects.toBeInstanceOf(MatomoApiError);
    await expect(service.getReferrers({ idSite: "1,4,5" })).rejects.toBeInstanceOf(MatomoApiError);
    expect(fetchMock.mock.calls.length).toBe(0);
  });

  it("Referrers.getAll accepts a single site and date", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    const result = await service.getReferrers({ idSite: 1, date: "yesterday" });

    expect(result.rows).toEqual([]);
    expect(paramsOf(fetchMock).get("method")).toBe("Referrers.getAll");
  });

  it("VisitTime.getByDayOfWeek refuses a range and multiple sites too", async () => {
    await expect(
      service.getVisitsByDayOfWeek({ idSite: 1, period: "range", date: "2026-09-01,2026-09-30" }),
    ).rejects.toBeInstanceOf(MatomoApiError);
    await expect(service.getVisitsByDayOfWeek({ idSite: "all" })).rejects.toBeInstanceOf(
      MatomoApiError,
    );
    expect(fetchMock.mock.calls.length).toBe(0);
  });

  it("getReferrerWebsites accepts a range, unlike getAll", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.getReferrerWebsites({
      idSite: 1,
      period: "range",
      date: "2026-09-01,2026-09-30",
    });

    expect(paramsOf(fetchMock).get("method")).toBe("Referrers.getWebsites");
  });

  it("getUrlsFromWebsiteId requires an integer idSubtable", async () => {
    await expect(
      service.getUrlsFromWebsiteId({ idSite: 1, idSubtable: 1.5 }),
    ).rejects.toBeInstanceOf(MatomoApiError);

    fetchMock.mockResolvedValueOnce(reply([]));
    await service.getUrlsFromWebsiteId({ idSite: 1, idSubtable: 12 });
    expect(paramsOf(fetchMock).get("idSubtable")).toBe("12");
  });

  /* ------------------------------------------------------------------ */
  /* Sites                                                              */
  /* ------------------------------------------------------------------ */

  it("parses SitesManager.getAllSites from the object-keyed-by-idsite shape", async () => {
    const body = {
      "1": {
        idsite: "1",
        name: "Example",
        description: "main",
        main_url: "https://example.test",
        ts_created: "2024-01-01 00:00:00",
        timezone: "UTC",
        currency: "USD",
        timezone_name: "UTC",
        currency_name: "US Dollar",
        creator_login: "admin",
      },
      "4": {
        idsite: "4",
        name: "Second",
        description: "",
        main_url: "https://second.test",
        ts_created: "2025-06-01 00:00:00",
        timezone: "Europe/Paris",
        currency: "EUR",
      },
    };
    fetchMock.mockImplementation(async () => reply(body));

    const raw = await service.listSitesRaw();

    expect(Object.keys(raw)).toEqual(["1", "4"]);
    expect(raw["1"]?.name).toBe("Example");
    expect(raw["4"]?.currency).toBe("EUR");
    // creator_login is superuser-only, so a row without it is not a failure.
    expect(raw["4"]?.creator_login).toBeUndefined();

    const list = await service.listSites();
    expect(list.map((site) => site.idsite)).toEqual(["1", "4"]);
  });

  it("reads the lower-case idsite and sends no idSite for SitesManager.getAllSitesId", async () => {
    fetchMock.mockResolvedValueOnce(reply([1, 4, 5]));

    await service.listSiteIds();

    const params = paramsOf(fetchMock);
    expect(params.get("method")).toBe("SitesManager.getAllSitesId");
    expect(params.has("idSite")).toBe(false);
    expect(params.has("period")).toBe(false);
  });

  it("treats an empty SitesManager.getAllSitesId as no ids rather than an error", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    expect(await service.listSiteIds()).toEqual([]);
  });

  it("reads the flat int[] from SitesManager.getAllSitesId", async () => {
    fetchMock.mockResolvedValueOnce(reply([1, 4, 5]));

    expect(await service.listSiteIds()).toEqual([1, 4, 5]);
  });

  it("reads the bare scalar from API.getMatomoVersion", async () => {
    fetchMock.mockResolvedValueOnce(reply("5.14.0"));

    expect(await service.getVersion()).toBe("5.14.0");
    expect(paramsOf(fetchMock).has("idSite")).toBe(false);
  });

  it("falls back to the restricted-token site endpoints on request", async () => {
    fetchMock.mockImplementation(async () =>
      reply({
        "7": {
          idsite: "7",
          name: "Restricted",
          description: "",
          main_url: "",
          ts_created: "",
          timezone: "",
          currency: "USD",
        },
      }),
    );

    const admin = await service.listSitesWithAdminAccess();
    expect(paramsOf(fetchMock).get("method")).toBe("SitesManager.getSitesWithAdminAccess");
    expect(admin).toHaveLength(1);

    fetchMock.mockClear();
    await service.listSitesWithViewAccess();
    expect(paramsOf(fetchMock).get("method")).toBe("SitesManager.getSitesWithViewAccess");
  });

  /* ------------------------------------------------------------------ */
  /* Optional columns                                                   */
  /* ------------------------------------------------------------------ */

  it("models nb_uniq_visitors and nb_users as optional — a period without them still parses", async () => {
    fetchMock.mockResolvedValueOnce(reply({ nb_visits: 5, nb_actions: 9, bounce_count: 1 }));

    const result = await service.getVisitsSummary({ idSite: 1 });

    const row = result.rows[0];
    expect(row?.nb_visits).toBe(5);
    expect(row?.nb_uniq_visitors).toBeUndefined();
    expect(row?.nb_users).toBeUndefined();
  });

  it("a VisitsSummary row typechecks without the unique-visitor columns", () => {
    // Compile-time assertion: these two fields are OPTIONAL, so a row that omits
    // them must satisfy MatomoVisitsSummaryRow. If they were required this
    // assignment would fail to compile.
    const withoutUnique: MatomoVisitsSummaryRow = {
      nb_visits: 1,
      nb_actions: 2,
      bounce_count: 0,
      sum_visit_length: 10,
      max_actions: 4,
    };
    expect(withoutUnique.nb_uniq_visitors).toBeUndefined();

    const withUnique: MatomoVisitsSummaryRow = {
      ...withoutUnique,
      nb_uniq_visitors: 3,
      nb_users: 3,
    };
    expect(withUnique.nb_uniq_visitors).toBe(3);
  });

  it("carries Referrers metadata flat on the row, and UserCountry code/logo beside label", async () => {
    fetchMock.mockResolvedValueOnce(
      reply([{ label: "example.com", nb_visits: 4, segment: "refererType=external" }]),
    );
    const referrers = await service.getReferrerWebsites({ idSite: 1 });
    expect(referrers.rows[0]?.segment).toBe("refererType=external");
    expect(referrers.rows[0]?.label).toBe("example.com");

    fetchMock.mockResolvedValueOnce(
      reply([{ label: "France", code: "fr", logo: "fr.png", nb_visits: 9 }]),
    );
    const countries = await service.getCountries({ idSite: 1 });
    // `label` is the TRANSLATED name; `code` is the stable key.
    expect(countries.rows[0]?.label).toBe("France");
    expect(countries.rows[0]?.code).toBe("fr");
    expect(countries.rows[0]?.logo).toBe("fr.png");
  });

  /* ------------------------------------------------------------------ */
  /* Metric coercion                                                    */
  /* ------------------------------------------------------------------ */

  it("coerces the formatted metrics the server produces when format_metrics is not honoured", () => {
    expect(matomoNumber(0.42)).toBe(0.42);
    expect(matomoNumber("42%")).toBe(42);
    expect(matomoNumber("1 min 23 sec")).toBe(83);
    expect(matomoNumber("2 hours 3 min")).toBe(7380);
    expect(matomoNumber("")).toBeUndefined();
    expect(matomoNumber(null)).toBeUndefined();
    expect(matomoNumber(undefined)).toBeUndefined();
    expect(matomoNumber("not a number")).toBeUndefined();
  });

  it("matomoPercent turns the 0-1 bounce_rate into 0-100 and leaves a formatted value alone", () => {
    expect(matomoPercent(0.42)).toBeCloseTo(42);
    expect(matomoPercent("42%")).toBeCloseTo(42);
    expect(matomoPercent(undefined)).toBeUndefined();
  });

  it("everything this adapter issues is a GET", async () => {
    fetchMock.mockImplementation(async () => reply({ nb_visits: 1 }));

    await service.getVisitsSummary({ idSite: 1 });
    await service.listSites();

    for (const call of fetchMock.mock.calls) {
      expect((call[1] as RequestInit | undefined)?.method ?? "GET").toBe("GET");
    }
  });
});

/* -------------------------------------------------------------------------- */
/* The shape guard, tested directly against all five arms                      */
/* -------------------------------------------------------------------------- */

describe("classifyMatomoResponse", () => {
  it("discriminates a flat object (single-period numeric archive)", () => {
    const classified = classifyMatomoResponse({
      nb_visits: 12,
      bounce_rate: 0.5,
      avg_time_on_site: 90,
    });
    expect(classified.kind).toBe("flat");
    if (classified.kind !== "flat") throw new Error("expected flat");
    expect(classified.row["nb_visits"]).toBe(12);
  });

  it("discriminates a date-keyed map", () => {
    const classified = classifyMatomoResponse({
      "2026-09-01": { nb_visits: 12 },
      "2026-09-02": { nb_visits: 8 },
    });
    expect(classified.kind).toBe("dates");
    if (classified.kind !== "dates") throw new Error("expected dates");
    expect(Object.keys(classified.dates)).toEqual(["2026-09-01", "2026-09-02"]);
  });

  it("discriminates a site-keyed then date-keyed map (two levels of nesting)", () => {
    const classified = classifyMatomoResponse({
      "Example Site": { "2026-09-01": { nb_visits: 3 }, "2026-09-02": { nb_visits: 4 } },
      "Second Site": { "2026-09-01": { nb_visits: 7 } },
    });
    expect(classified.kind).toBe("sites");
    if (classified.kind !== "sites") throw new Error("expected sites");
    expect(Object.keys(classified.sites)).toEqual(["Example Site", "Second Site"]);
    expect(Object.keys(classified.sites["Second Site"] ?? {})).toEqual(["2026-09-01"]);
  });

  it("reads a multi-site SINGLE-date map as site-keyed, not date-keyed", () => {
    // No nested objects, and the keys are site names rather than period labels.
    const classified = classifyMatomoResponse({
      "Example Site": { nb_visits: 3 },
      "Second Site": { nb_visits: 4 },
    });
    expect(classified.kind).toBe("sites");
    if (classified.kind !== "sites") throw new Error("expected sites");
    expect(Object.keys(classified.sites)).toEqual(["Example Site", "Second Site"]);
  });

  it("discriminates a flat array of row objects (a dimension report)", () => {
    const classified = classifyMatomoResponse([
      { label: "/a", nb_visits: 3 },
      { label: "/b", nb_visits: 4 },
    ]);
    expect(classified.kind).toBe("rows");
    if (classified.kind !== "rows") throw new Error("expected rows");
    expect(classified.rows).toHaveLength(2);
  });

  it("discriminates a bare scalar and a scalar array", () => {
    expect(classifyMatomoResponse("5.14.0")).toEqual({ kind: "scalar", value: "5.14.0" });
    expect(classifyMatomoResponse(42)).toEqual({ kind: "scalar", value: 42 });
    expect(classifyMatomoResponse(null)).toEqual({ kind: "scalar", value: null });

    const ids = classifyMatomoResponse([1, 4, 5]);
    expect(ids.kind).toBe("scalar");
    if (ids.kind !== "scalar") throw new Error("expected scalar");
    expect(ids.value).toEqual([1, 4, 5]);
  });

  it("does not mistake the SitesManager.getAllSites map for a date-keyed map", () => {
    const classified = classifyMatomoResponse({
      "1": { idsite: "1", name: "Example" },
      "4": { idsite: "4", name: "Second" },
    });
    expect(classified.kind).toBe("sites");
  });

  it("handles month and year period labels as date keys", () => {
    expect(classifyMatomoResponse({ "2026-09": { nb_visits: 1 } }).kind).toBe("dates");
    expect(classifyMatomoResponse({ "2026": { nb_visits: 1 } }).kind).toBe("dates");
    expect(classifyMatomoResponse({ "2026-09-01 14:00": { nb_visits: 1 } }).kind).toBe("dates");
  });
});

describe("normaliseMatomoResponse", () => {
  it("flattens every arm into {rows} and only fills dates/sites where they exist", () => {
    expect(normaliseMatomoResponse({ nb_visits: 1 })).toEqual({ rows: [{ nb_visits: 1 }] });
    expect(normaliseMatomoResponse([{ label: "/a" }])).toEqual({ rows: [{ label: "/a" }] });
    expect(normaliseMatomoResponse("5.14.0")).toEqual({ rows: [] });
    expect(normaliseMatomoResponse(null)).toEqual({ rows: [] });

    const dates = normaliseMatomoResponse({
      "2026-09-02": { nb_visits: 2 },
      "2026-09-01": { nb_visits: 1 },
    });
    expect(dates.dates).toEqual(["2026-09-01", "2026-09-02"]);
    expect(dates.rows.map((row) => row.date)).toEqual(["2026-09-01", "2026-09-02"]);
    expect(dates.sites).toBeUndefined();

    const sites = normaliseMatomoResponse({
      "Site A": { "2026-09-02": { nb_visits: 2 }, "2026-09-01": { nb_visits: 1 } },
    });
    expect(sites.sites).toEqual(["Site A"]);
    // The date axis is per-site, so a site-keyed result reports no global `dates`.
    expect(sites.dates).toBeUndefined();
    expect(sites.rows.map((row) => [row.siteId, row.date, row.nb_visits])).toEqual([
      ["Site A", "2026-09-01", 1],
      ["Site A", "2026-09-02", 2],
    ]);
  });

  it("treats an empty object as no rows rather than one empty row", () => {
    expect(normaliseMatomoResponse({})).toEqual({ rows: [] });
  });

  it("gives a multi-site single-date map one row per site", () => {
    const result = normaliseMatomoResponse({
      "Site A": { nb_visits: 3 },
      "Site B": { nb_visits: 4 },
    });
    expect(result.rows).toEqual([
      { nb_visits: 3, siteId: "Site A" },
      { nb_visits: 4, siteId: "Site B" },
    ]);
    expect(result.dates).toBeUndefined();
  });
});

/* -------------------------------------------------------------------------- */
/* Manifest                                                                    */
/* -------------------------------------------------------------------------- */

describe("matomo manifest", () => {
  it("declares the manifest contract", () => {
    expect(matomoManifest.id).toBe("matomo");
    expect(matomoManifest.uiPath).toBe("/plugins/matomo");
    expect(matomoManifest.upstream).toEqual({ product: "Matomo", envPrefix: "MATOMO" });
    expect(matomoManifest.agent?.skills.length).toBeGreaterThan(0);
    for (const skill of matomoManifest.agent?.skills ?? []) {
      expect(skill.id).toMatch(/^[a-zA-Z][a-zA-Z0-9]*$/);
      expect(skill.description.length).toBeGreaterThan(0);
    }
  });

  it("pairs every declared skill with a loader handler", () => {
    const declared = (matomoManifest.agent?.skills ?? []).map((skill) => skill.id);
    // `PluginLoader.skills` takes a runtime the handlers ignore here, so a bare
    // stub is enough to enumerate them.
    const handled = (matomoLoader.skills?.(RUNTIME_STUB) ?? []).map((skill) => skill.id);
    expect(handled).toEqual(declared);
  });

  it("documents manual token provisioning and the Bearer route", () => {
    expect(matomoManifest.agent?.description).toMatch(/Bearer/);
    expect(matomoManifest.agent?.description).toMatch(/format_metrics=0/);
  });
});
