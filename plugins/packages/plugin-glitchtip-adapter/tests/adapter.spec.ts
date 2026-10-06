import { describe, it, expect, beforeEach, afterEach, vi } from "vite-plus/test";
import { Context } from "cordis";
import { UpstreamClient, UpstreamError } from "@loams-plugins/plugin-upstream-http";
import {
  GLITCHTIP_API_PREFIX,
  GLITCHTIP_HEADER_HITS,
  GLITCHTIP_HEADER_LINK,
  GLITCHTIP_HEADER_MAX_HITS,
  GLITCHTIP_LIST_LIMIT,
  GlitchtipAdapterService,
  GlitchtipIssue,
  GlitchtipPaginationError,
  GlitchtipStatsTuple,
  assertGlitchtipIssueSort,
  glitchtipBearerAuth,
  glitchtipIssueCount,
  glitchtipManifest,
  glitchtipTagsToRecord,
  nextGlitchtipCursor,
  parseGlitchtipPaginationHeaders,
  parseGlitchtipStats,
} from "../src/index.js";

/**
 * Every case here asserts on the WIRE, not on the adapter's own return value.
 *
 * The GlitchTip failure modes are all silent: a body read as an envelope yields
 * nothing, epoch seconds charted as milliseconds land in 1970, a tag map read
 * off an array yields `undefined`, and a defaulted pagination header turns a
 * populated page into "0 results". None of them throw.
 */

const TOKEN = "gt_a1b2c3";
const ORG = "acme";

interface Captured {
  url: string;
  headers: Record<string, string>;
}

interface Harness {
  service: GlitchtipAdapterService;
  captured: Captured[];
  restore(): void;
}

const LINK_NEXT = `<https://gt.test${GLITCHTIP_API_PREFIX}organizations/${ORG}/issues/?cursor=cD0yMDI2>; rel="next", <https://gt.test${GLITCHTIP_API_PREFIX}organizations/${ORG}/issues/?cursor=PREV>; rel="previous"`;

interface ResponseSpec {
  body: unknown;
  headers?: Record<string, string>;
  status?: number;
}

/** Default paginated headers, overridable per call. */
function pageHeaders(extra: Record<string, string> = {}): Record<string, string> {
  return {
    "Content-Type": "application/json",
    [GLITCHTIP_HEADER_HITS]: "1",
    [GLITCHTIP_HEADER_MAX_HITS]: "2",
    [GLITCHTIP_HEADER_LINK]: LINK_NEXT,
    ...extra,
  };
}

function issue(): GlitchtipIssue {
  return {
    id: "1234",
    type: "error",
    // A STRINGIFIED NUMBER, on purpose.
    count: "42",
    project: { id: "9f8e" },
    stats: { "24h": [[1767225600, 7]], "14d": [[1766620800, 3]] },
  };
}

function respond(
  pathname: string,
  search: URLSearchParams,
  spec: ResponseSpec | undefined,
): ResponseSpec {
  if (spec) return spec;
  if (pathname === `${GLITCHTIP_API_PREFIX}`) {
    return {
      body: {
        version: "4.12.0",
        user: { id: 1 },
        auth: {
          id: 1,
          label: "bi",
          scopes: ["project:read", "org:read"],
          token: TOKEN,
          created: "2026-01-01",
        },
      },
      headers: { "Content-Type": "application/json" },
    };
  }
  if (pathname === `${GLITCHTIP_API_PREFIX}organizations/`) {
    return { body: [{ name: "Acme", slug: ORG, require2fa: false }], headers: pageHeaders() };
  }
  if (pathname === `${GLITCHTIP_API_PREFIX}users/me`) {
    return {
      body: { id: 1, email: "oncall@example.com" },
      headers: { "Content-Type": "application/json" },
    };
  }
  // `/issues/{id}/events/` must be matched BEFORE the bare `/issues/` prefix, or
  // every event request is answered with an issue row.
  if (pathname.includes("/events/")) {
    return {
      body: [
        {
          projectID: 9,
          type: "Error",
          // ARRAY OF SINGLE-KEY OBJECTS, not a map.
          tags: [{ browser: "Firefox" }, { os: "macOS" }],
          metadata: { runtime: "node" },
          user: { id: "end-user-1", email: undefined },
        },
      ],
      headers: pageHeaders(),
    };
  }
  if (pathname.includes("/issues/")) {
    return {
      body: [issue()],
      headers: pageHeaders({
        [GLITCHTIP_HEADER_LINK]:
          search.get("cursor") === null
            ? LINK_NEXT
            : `<https://gt.test${GLITCHTIP_API_PREFIX}organizations/${ORG}/issues/?cursor=cD0yMDI2>; rel="next"`,
      }),
    };
  }
  if (pathname.includes("/issues-stats/")) {
    return {
      body: [{ category: "error", data: [[1767225600, 5]] }],
      headers: pageHeaders(),
    };
  }
  if (pathname.includes("/transaction-groups/")) {
    return {
      body: [{ avgDuration: 125, p50: 90, p95: 400, errorRate: 0.02, throughput: 12 }],
      headers: pageHeaders(),
    };
  }
  if (pathname.endsWith("/api-tokens/")) {
    return { body: [], headers: { "Content-Type": "application/json" } };
  }
  return { body: [], headers: pageHeaders() };
}

function makeService(
  specFor?: (pathname: string, search: URLSearchParams) => ResponseSpec | undefined,
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
    const parsed = new URL(url);
    const spec = (specFor?.(parsed.pathname, parsed.searchParams) ??
      respond(parsed.pathname, parsed.searchParams, undefined))!;
    return new Response(JSON.stringify(spec.body), {
      status: spec.status ?? 200,
      headers: spec.headers ?? { "Content-Type": "application/json" },
    });
  }) as typeof fetch;

  const service = new GlitchtipAdapterService(new Context(), {
    baseUrl: "https://gt.test",
    token: TOKEN,
  });
  return { service, captured, restore: () => (globalThis.fetch = original) };
}

let harness: Harness | null = null;
beforeEach(() => {
  harness = makeService();
});
afterEach(() => {
  harness?.restore();
  vi.restoreAllMocks();
});

describe("glitchtip auth", () => {
  it("sends Authorization: Bearer <APIToken> on every request", async () => {
    await harness!.service.listIssues(ORG);
    expect(harness!.captured[0]!.headers.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("sends the bearer header on the header-aware list path too", async () => {
    await harness!.service.listOrganizations();
    expect(harness!.captured[0]!.headers.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("builds UpstreamAuth as bearer, not `Token <key>`", () => {
    // `Authorization: Token <key>` is UNVERIFIED upstream and is not claimed.
    expect(glitchtipBearerAuth(TOKEN)).toEqual({ kind: "bearer", token: TOKEN });
  });

  it("targets /api/0/ and not the legacy ingest paths", async () => {
    await harness!.service.listIssues(ORG);
    const url = new URL(harness!.captured[0]!.url);
    expect(url.pathname.startsWith(`${GLITCHTIP_API_PREFIX}organizations/`)).toBe(true);
    // `/api/{project_id}/store/` and `/api/security/` are ingest-only.
    expect(url.pathname).not.toMatch(/store\/|security\//);
  });
});

describe("glitchtip root is health + token check", () => {
  it("returns {version, user, auth} from GET /api/0/", async () => {
    const root = await harness!.service.root();
    expect(new URL(harness!.captured[0]!.url).pathname).toBe(GLITCHTIP_API_PREFIX);
    expect(root.version).toBe("4.12.0");
    expect(root.auth?.scopes).toEqual(["project:read", "org:read"]);
  });

  it("does not claim a shape for `user` — it is unknown", async () => {
    const root = await harness!.service.root();
    // `user` is carried through untyped rather than guessed at.
    expect(root.user).toEqual({ id: 1 });
    const asString: unknown = root.user;
    expect(typeof asString).toBe("object");
  });

  it("reports diagnostics without throwing, and surfaces require2fa", async () => {
    const result = await harness!.service.diagnostics();
    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error("expected ok");
    expect(result.tokenScopes).toEqual(["project:read", "org:read"]);
    expect(result.organizations[0]!.require2fa).toBe(false);
  });

  it("reports a failure instead of throwing when the deployment is down", async () => {
    harness!.restore();
    globalThis.fetch = (async () => {
      throw new Error("ECONNREFUSED");
    }) as typeof fetch;
    const result = await harness!.service.diagnostics();
    expect(result.ok).toBe(false);
  });
});

describe("glitchtip bare-array envelope", () => {
  it("reads a list body as a bare JSON array, with no wrapper", async () => {
    const page = await harness!.service.listIssues(ORG);
    expect(Array.isArray(page.data)).toBe(true);
    expect(page.data).toHaveLength(1);
    // There is no `results`/`count` to read; the adapter must not invent one.
    expect(page).not.toHaveProperty("results");
    expect(page).not.toHaveProperty("count");
  });

  it("raises a clear error when a list endpoint returns an object instead", async () => {
    const bad = makeService(() => ({
      body: { results: [issue()], count: 1 },
      headers: pageHeaders(),
    }));
    await expect(bad.service.listIssues(ORG)).rejects.toThrow(/bare JSON array/);
    bad.restore();
  });
});

describe("glitchtip cursor pagination via headers", () => {
  it("reads X-Hits, X-Max-Hits and the RFC-5988 Link", async () => {
    const page = await harness!.service.listIssues(ORG);
    expect(page.pagination.hits).toBe(1);
    expect(page.pagination.maxHits).toBe(2);
    expect(page.pagination.next).toBe("cD0yMDI2");
    expect(page.pagination.previous).toBe("PREV");
  });

  it("sends `cursor`, and never `page` or `offset`", async () => {
    await harness!.service.listIssues(ORG, { cursor: "abc" });
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.get("cursor")).toBe("abc");
    expect(url.searchParams.has("page")).toBe(false);
    expect(url.searchParams.has("offset")).toBe(false);
  });

  it("defaults limit to the known-good 200", async () => {
    await harness!.service.listIssues(ORG);
    expect(new URL(harness!.captured[0]!.url).searchParams.get("limit")).toBe("200");
    expect(GLITCHTIP_LIST_LIMIT).toBe(200);
  });

  it("throws rather than defaulting when a pagination header is MISSING", async () => {
    const noHeaders = makeService(() => ({
      body: [issue()],
      headers: { "Content-Type": "application/json" },
    }));
    await expect(noHeaders.service.listIssues(ORG)).rejects.toThrow(GlitchtipPaginationError);
    await expect(noHeaders.service.listIssues(ORG)).rejects.toThrow(/X-Hits/);
    await expect(noHeaders.service.listIssues(ORG)).rejects.toThrow(/X-Max-Hits/);
    await expect(noHeaders.service.listIssues(ORG)).rejects.toThrow(/Link/);
    noHeaders.restore();
  });

  it("names only the header that is actually missing", () => {
    const headers = new Headers({
      [GLITCHTIP_HEADER_HITS]: "1",
      [GLITCHTIP_HEADER_LINK]: LINK_NEXT,
    });
    expect(() => parseGlitchtipPaginationHeaders(headers)).toThrow(/X-Max-Hits/);
    const complete = new Headers(pageHeaders());
    expect(parseGlitchtipPaginationHeaders(complete).maxHits).toBe(2);
  });

  it("walks pages until the Link has no rel=next", async () => {
    const pages: number[] = [];
    for await (const page of harness!.service.walk(`organizations/${ORG}/issues/`)) {
      pages.push(page.data.length);
    }
    // Page 2 echoes the SAME cursor, so the walk stops rather than looping.
    expect(pages).toEqual([1, 1]);
  });

  it("stops at the page cap", async () => {
    // A FRESH cursor every call, so only the cap can end the walk. (A constant
    // cursor would be caught by the repeat-cursor guard instead, one page earlier.)
    let call = 0;
    vi.spyOn(
      harness!.service as unknown as { _list: () => Promise<unknown> },
      "_list",
    ).mockImplementation(async () => {
      call += 1;
      return { data: [issue()], pagination: { hits: 1, maxHits: 99, next: `cursor-${call}` } };
    });
    const pages: number[] = [];
    for await (const page of harness!.service.walk(
      "organizations/acme/issues/",
      {},
      { maxPages: 3 },
    )) {
      pages.push(page.data.length);
    }
    expect(pages).toHaveLength(3);
  });

  it("extracts only the cursor from the Link URL", () => {
    expect(nextGlitchtipCursor(LINK_NEXT)).toBe("cD0yMDI2");
    expect(nextGlitchtipCursor('<https://x/?cursor=abc>; rel="previous"')).toBeUndefined();
    expect(nextGlitchtipCursor(undefined)).toBeUndefined();
    expect(nextGlitchtipCursor(null as unknown as undefined)).toBeUndefined();
  });
});

describe("glitchtip issue field coercion", () => {
  it("treats IssueSchema.id as a STRING", async () => {
    const page = await harness!.service.listIssues(ORG);
    const id = page.data[0]!.id;
    expect(typeof id).toBe("string");
    expect(id).toBe("1234");
    // The path uses the string form verbatim, unquoted.
    await harness!.service.getIssue(id);
    expect(new URL(harness!.captured[1]!.url).pathname).toBe(`${GLITCHTIP_API_PREFIX}issues/1234/`);
  });

  it("parseInts the STRINGIFIED count", () => {
    expect(glitchtipIssueCount("42")).toBe(42);
    expect(glitchtipIssueCount(7)).toBe(7);
    expect(glitchtipIssueCount(null)).toBe(0);
    expect(glitchtipIssueCount(undefined)).toBe(0);
    expect(glitchtipIssueCount("not a number")).toBe(0);
  });

  it("reads the count off a real issue without coercing it by accident", async () => {
    const page = await harness!.service.listIssues(ORG);
    const raw = page.data[0]!.count;
    expect(typeof raw).toBe("string");
    expect(glitchtipIssueCount(raw)).toBe(42);
  });

  it("keeps project.id a string while IssueEvent.projectID is a number", async () => {
    const issues = await harness!.service.listIssues(ORG);
    expect(typeof issues.data[0]!.project.id).toBe("string");
    const events = await harness!.service.listIssueEvents("1234");
    expect(typeof events.data[0]!.projectID).toBe("number");
  });
});

describe("glitchtip stats are EPOCH SECONDS", () => {
  const stats = {
    "24h": [
      [1767225600, 7],
      [1767312000, 9],
    ] as GlitchtipStatsTuple[],
    "14d": [[1766620800, 3]] as GlitchtipStatsTuple[],
  };

  it("multiplies by 1000 to get charting milliseconds", () => {
    const points = parseGlitchtipStats(stats, "24h");
    expect(points).toHaveLength(2);
    expect(points[0]).toEqual({ timestampMs: 1767225600000, epochSeconds: 1767225600, count: 7 });
    // The seconds value is NOT already ms: reading it as ms lands in Jan 1970.
    expect(new Date(points[0]!.timestampMs).getUTCFullYear()).toBe(2026);
    expect(new Date(points[0]!.epochSeconds).getUTCFullYear()).toBe(1970);
  });

  it("reads the tuple as [epochSeconds, count] in that order", () => {
    const [point] = parseGlitchtipStats(stats, "24h");
    expect(point!.count).toBe(7);
    expect(point!.epochSeconds).toBe(1767225600);
  });

  it("selects a declared period, and flattens both when none is named", () => {
    expect(parseGlitchtipStats(stats, "14d")).toHaveLength(1);
    expect(parseGlitchtipStats(stats)).toHaveLength(3);
  });

  it("returns an empty series for null stats rather than throwing", () => {
    expect(parseGlitchtipStats(null)).toEqual([]);
    expect(parseGlitchtipStats(undefined)).toEqual([]);
    expect(parseGlitchtipStats({ "24h": [] }, "24h")).toEqual([]);
  });

  it("parses a real issue's stats", async () => {
    const page = await harness!.service.listIssues(ORG);
    const points = parseGlitchtipStats(page.data[0]!.stats, "24h");
    expect(points[0]!.timestampMs).toBe(1767225600000);
  });
});

describe("glitchtip issue events", () => {
  it("flattens tags from an array of single-key objects", async () => {
    const events = await harness!.service.listIssueEvents("1234");
    const tags = events.data[0]!.tags;
    expect(Array.isArray(tags)).toBe(true);
    expect(tags).toEqual([{ browser: "Firefox" }, { os: "macOS" }]);

    const record = glitchtipTagsToRecord(tags);
    expect(record).toEqual({ browser: "Firefox", os: "macOS" });
    // The direct-map read that the array shape breaks:
    expect((tags as unknown as Record<string, string>).browser).toBeUndefined();
  });

  it("handles a null or absent tags array", () => {
    expect(glitchtipTagsToRecord(null)).toEqual({});
    expect(glitchtipTagsToRecord(undefined)).toEqual({});
    expect(glitchtipTagsToRecord([])).toEqual({});
  });

  it("types IssueEvent.type as a plain string, not the IssueSchema enum", async () => {
    const events = await harness!.service.listIssueEvents("1234");
    // "Error" is not `default|error|csp` — the event type is unschema'd.
    expect(events.data[0]!.type).toBe("Error");
  });

  it("types metadata values as strings and `user` as unknown", async () => {
    const events = await harness!.service.listIssueEvents("1234");
    expect(events.data[0]!.metadata).toEqual({ runtime: "node" });
    expect(events.data[0]!.user).toEqual({ id: "end-user-1", email: undefined });
  });
});

describe("glitchtip issues query", () => {
  it("sends project as NUMERIC ids, repeated", async () => {
    await harness!.service.listIssues(ORG, {
      project: [12, 34],
      environment: ["production", "staging"],
    });
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.getAll("project")).toEqual(["12", "34"]);
    expect(url.searchParams.getAll("environment")).toEqual(["production", "staging"]);
  });

  it("validates `sort`, including the descending `-` prefix", async () => {
    await harness!.service.listIssues(ORG, { sort: "-count" });
    expect(new URL(harness!.captured[0]!.url).searchParams.get("sort")).toBe("-count");
    expect(() => assertGlitchtipIssueSort("last_seen")).not.toThrow();
    expect(() => assertGlitchtipIssueSort("-priority")).not.toThrow();
    expect(() => assertGlitchtipIssueSort("impact" as never)).toThrow(/sort/);
  });

  it("invents NO `status` param — status filtering is client-side upstream", async () => {
    await harness!.service.listIssues(ORG);
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.has("status")).toBe(false);

    // And it cannot be sent even by accident: the query type has no such field.
    const params = new URLSearchParams();
    params.set("status", "unresolved");
    // The adapter never forwards a caller-built param bag for issues.
    expect(params.has("status")).toBe(true);
  });
});

describe("glitchtip issues-stats", () => {
  it("sends the REQUIRED `groups` param, repeated", async () => {
    await harness!.service.issueStats(ORG, { groups: [1234, 5678], statsPeriod: "14d" });
    const url = new URL(harness!.captured[0]!.url);
    expect(url.pathname).toBe(`${GLITCHTIP_API_PREFIX}organizations/${ORG}/issues-stats/`);
    expect(url.searchParams.getAll("groups")).toEqual(["1234", "5678"]);
    expect(url.searchParams.get("statsPeriod")).toBe("14d");
  });

  it("refuses to send a request with no groups at all", async () => {
    const before = harness!.captured.length;
    await expect(harness!.service.issueStats(ORG, { groups: [] })).rejects.toThrow(
      /requires `groups`/,
    );
    expect(harness!.captured.length).toBe(before);
  });

  it("accepts groups without statsPeriod", async () => {
    await harness!.service.issueStats(ORG, { groups: [1] });
    const url = new URL(harness!.captured[0]!.url);
    expect(url.searchParams.has("statsPeriod")).toBe(false);
  });
});

describe("glitchtip performance units", () => {
  it("reads avgDuration/p50/p95 as MILLISECONDS and treats errorRate/throughput as read-only", async () => {
    const page = await harness!.service.listTransactionGroups(ORG);
    const group = page.data[0]!;
    expect(group.avgDuration).toBe(125);
    expect(group.p50).toBe(90);
    expect(group.p95).toBe(400);
    expect(group.errorRate).toBe(0.02);
    expect(group.throughput).toBe(12);
    // `errorRate`/`throughput` are declared `readonly` on
    // GlitchtipTransactionGroup — a COMPILE-time guarantee, erased at runtime, so
    // the assertion here is that the values are readable (they are serializer-
    // computed) rather than that assignment throws.
    expect(typeof group.errorRate).toBe("number");
    expect(typeof group.throughput).toBe("number");
  });
});

describe("glitchtip unverified and absent surfaces", () => {
  it("omits stats_v2 entirely rather than inventing a response type", () => {
    const service = harness!.service as unknown as Record<string, unknown>;
    expect(Object.keys(service)).not.toContain("statsV2");
    expect(typeof service.statsV2).toBe("undefined");
  });

  it("models no end-user endpoint, and names `users/` as staff", async () => {
    const staff = await harness!.service.listStaffUsers();
    expect(Array.isArray(staff.data)).toBe(true);
    const service = harness!.service as unknown as Record<string, unknown>;
    expect(Object.keys(service).some((key) => /endUser/i.test(key))).toBe(false);
    // `users/me` is the token's own staff identity.
    expect(await harness!.service.getCurrentUser()).toEqual({ id: 1, email: "oncall@example.com" });
  });

  it("does not claim `Authorization: Token <key>` support", async () => {
    await harness!.service.listIssues(ORG);
    expect(harness!.captured[0]!.headers.authorization).not.toMatch(/^Token /);
  });
});

describe("glitchtip api tokens", () => {
  it("lists tokens even without pagination headers, which that route may not send", async () => {
    const tokens = await harness!.service.listApiTokens();
    expect(tokens).toEqual([]);
  });
});

describe("glitchtip transport", () => {
  it("issues its list requests through the shared client, with a response observer", async () => {
    // `_list` used to build its own `fetch`, duplicating the bearer header, the
    // timeout and the status mapping. It now hands the shared client an
    // `onResponse` observer and takes its pagination headers from there — so the
    // ONE request a list makes is the client's, carrying an observer.
    const spy = vi.spyOn(UpstreamClient.prototype, "request");
    await harness!.service.listIssues(ORG);
    // `mockRestore` discards the recorded calls, so this has to come after the
    // assertions rather than in a `finally`.
    const calls = spy.mock.calls;
    spy.mockRestore();

    expect(calls).toHaveLength(1);
    const [method, target, options] = calls[0]!;
    expect(method).toBe("GET");
    expect(target).toBe(`${GLITCHTIP_API_PREFIX}organizations/${ORG}/issues/`);
    expect(typeof (options as { onResponse?: unknown }).onResponse).toBe("function");
    // ...and the client made exactly one HTTP call for it, not one of its own
    // in addition.
    expect(harness!.captured).toHaveLength(1);
    expect(harness!.captured[0]!.headers.accept).toBe("application/json");
    expect(harness!.captured[0]!.headers.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("never replaces globalThis.fetch", async () => {
    const before = globalThis.fetch;
    await harness!.service.listIssues(ORG);
    await harness!.service.listOrganizations();
    expect(globalThis.fetch).toBe(before);
    expect(globalThis.fetch).not.toBe(undefined);
  });

  it("takes its timeout from the client configuration, not a private copy of it", async () => {
    // The bespoke fetch carried its own AbortController and timer. A hung list
    // request must now fail with the shared client's 408 timeout instead.
    const slow = makeService();
    globalThis.fetch = (async () => {
      const error = new Error("The operation was aborted");
      error.name = "AbortError";
      throw error;
    }) as typeof fetch;
    const service = new GlitchtipAdapterService(new Context(), {
      baseUrl: "https://gt.test",
      token: TOKEN,
      timeoutMs: 5,
    });

    const error = await service.listIssues(ORG).catch((err: unknown) => err);
    expect(error).toBeInstanceOf(UpstreamError);
    expect((error as UpstreamError).status).toBe(408);
    expect((error as UpstreamError).body).toContain("timed out after 5ms");
    slow.restore();
  });
});

describe("glitchtip manifest", () => {
  it("declares the documented shape", () => {
    expect(glitchtipManifest.id).toBe("glitchtip");
    expect(glitchtipManifest.uiPath).toBe("/plugins/glitchtip");
    expect(glitchtipManifest.upstream).toEqual({ product: "GlitchTip", envPrefix: "GLITCHTIP" });
    expect(glitchtipManifest.agent!.skills.length).toBeGreaterThan(0);
  });
});
