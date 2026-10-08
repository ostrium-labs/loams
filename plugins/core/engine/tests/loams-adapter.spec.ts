import { describe, it, expect, vi, beforeEach, afterEach, type Mock } from "vite-plus/test";
import { Context } from "cordis";
import { AgentBus, HttpRouter, type PluginManifest, type PluginRuntime } from "@loams-core/host";
import {
  LOAMS_API_PREFIX,
  LOAMS_DEFAULT_LIMIT,
  LOAMS_DEFAULT_RRF_K,
  LOAMS_NO_NAMESPACE_MESSAGE,
  LoamsAdapterService,
  LoamsApiError,
  buildSearchRequest,
  loamsLoader,
  loamsManifest,
} from "../src/service.js";
import type { LoamsConfig, LoamsRetriever } from "../src/types.js";

/**
 * A `fetch` spy. `Mock<typeof fetch>` (rather than `ReturnType<typeof vi.fn<...>>`)
 * keeps `mock.calls` typed as fetch's real parameter tuple, so reading the
 * `RequestInit` off a call is checked instead of assumed.
 */
type MockedFetch = Mock<typeof fetch>;

/**
 * `PluginLoader.skills` is handed a `PluginRuntime`. These loaders build their
 * handler list from a literal and never read the runtime, but the contract
 * requires one, so give them a real (empty) one rather than casting `undefined`.
 */
function runtimeFor(manifest: PluginManifest): PluginRuntime {
  const ctx = new Context();
  return { id: manifest.id, manifest, ctx, router: new HttpRouter(ctx), bus: new AgentBus(ctx) };
}

const BASE = "http://loams.test:8080";
const NS = "demo";

function config(overrides: Partial<LoamsConfig> = {}): LoamsConfig {
  return { baseUrl: BASE, namespace: NS, ...overrides };
}

/** A JSON reply for the shared client. */
function reply(body: unknown, status = 200): Response {
  return new Response(body === undefined ? "" : JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** The URL the nth (0-based) fetch was called with. */
function urlOf(fetchMock: MockedFetch, index = 0): string {
  return String(fetchMock.mock.calls[index]?.[0]);
}

/** The decoded JSON body of the nth fetch. */
function bodyOf(fetchMock: MockedFetch, index = 0): Record<string, unknown> {
  const init = fetchMock.mock.calls[index]?.[1] as RequestInit | undefined;
  return JSON.parse(String(init?.body)) as Record<string, unknown>;
}

/** A service plus a fetch mock that answers `body` to every call. */
function serviceWith(body: unknown, overrides: Partial<LoamsConfig> = {}, status = 200) {
  const ctx = new Context();
  const service = new LoamsAdapterService(ctx, config(overrides));
  const fetchMock = vi.fn<typeof fetch>(async () => reply(body, status));
  vi.stubGlobal("fetch", fetchMock);
  return { ctx, service, fetchMock: fetchMock };
}

const TEXT_RETRIEVER: LoamsRetriever = {
  text: { query: { match: { field: "body", text: "refund" } }, k: 10 },
};

describe("base path and namespace interpolation", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn<typeof fetch>(async () => reply({})),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("interpolates the configured namespace into the /v1 prefix", async () => {
    const { service, fetchMock } = serviceWith({ collections: [] });
    await service.listCollections();
    expect(urlOf(fetchMock)).toBe(`${BASE}/v1/namespaces/${NS}/collections`);
  });

  it("percent-encodes a namespace that needs it", async () => {
    const { service, fetchMock } = serviceWith({ collections: [] }, { namespace: "team a/b" });
    await service.listCollections();
    expect(urlOf(fetchMock)).toBe(`${BASE}/v1/namespaces/team%20a%2Fb/collections`);
  });

  it("trims a trailing slash off the base URL", async () => {
    const { service, fetchMock } = serviceWith({ collections: [] }, { baseUrl: `${BASE}///` });
    await service.listCollections();
    expect(urlOf(fetchMock)).toBe(`${BASE}/v1/namespaces/${NS}/collections`);
  });

  it("percent-encodes a collection name in the path", async () => {
    const { service, fetchMock } = serviceWith({ id: 1 });
    await service.describeCollection("my kb/v2");
    expect(urlOf(fetchMock)).toBe(`${BASE}/v1/namespaces/${NS}/collections/my%20kb%2Fv2`);
  });

  it("hits the unversioned liveness routes", async () => {
    const { service, fetchMock } = serviceWith(undefined);
    await service.health();
    await service.ready();
    expect(urlOf(fetchMock, 0)).toBe(`${BASE}/health`);
    expect(urlOf(fetchMock, 1)).toBe(`${BASE}/ready`);
  });

  it("builds every document route under the collection path", async () => {
    const { service, fetchMock } = serviceWith({ documents: [], count: 0 });
    await service.getDocuments("kb", [1]);
    await service.scrollDocuments("kb");
    await service.countDocuments("kb");
    expect(urlOf(fetchMock, 0)).toBe(`${BASE}/v1/namespaces/${NS}/collections/kb/documents/get`);
    expect(urlOf(fetchMock, 1)).toBe(`${BASE}/v1/namespaces/${NS}/collections/kb/documents/scroll`);
    expect(urlOf(fetchMock, 2)).toBe(`${BASE}/v1/namespaces/${NS}/collections/kb/documents/count`);
  });
});

describe("namespace resolution", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn<typeof fetch>(async () => reply({})),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("resolves the configured namespace", async () => {
    const { service, fetchMock } = serviceWith({ collections: [] }, { namespace: "acme" });
    await service.listCollections();
    expect(urlOf(fetchMock)).toBe(`${BASE}/v1/namespaces/acme/collections`);
  });

  it("refuses to guess a namespace when none is configured", async () => {
    const ctx = new Context();
    const service = new LoamsAdapterService(ctx, { baseUrl: BASE });
    const fetchMock = vi.fn<typeof fetch>(async () => reply({}));
    vi.stubGlobal("fetch", fetchMock);

    await expect(service.listCollections()).rejects.toThrow(LOAMS_NO_NAMESPACE_MESSAGE);
    // The failure names the reason: the API has no list-namespaces route.
    expect(LOAMS_NO_NAMESPACE_MESSAGE).toMatch(/cannot discover one/);
    expect(LOAMS_NO_NAMESPACE_MESSAGE).toMatch(/api\/mod\.rs:114/);
    // Nothing was sent: a guessed namespace is worse than a refused call.
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses an empty namespace string too", async () => {
    const ctx = new Context();
    const service = new LoamsAdapterService(ctx, { baseUrl: BASE, namespace: "" });
    await expect(service.listCollections()).rejects.toThrow(LOAMS_NO_NAMESPACE_MESSAGE);
  });
});

describe("SearchRequest wire shape", () => {
  it("defaults limit to 10", () => {
    expect(buildSearchRequest({ collection: "kb" }).limit).toBe(LOAMS_DEFAULT_LIMIT);
    expect(LOAMS_DEFAULT_LIMIT).toBe(10);
  });

  it("defaults consistency to strong", () => {
    expect(buildSearchRequest({ collection: "kb" }).consistency).toBe("strong");
  });

  it("leaves fusion ABSENT unless asked for — absent means no fusion, not RRF", () => {
    const request = buildSearchRequest({ collection: "kb", retrievers: [TEXT_RETRIEVER] });
    expect(request).not.toHaveProperty("fusion");
    expect(JSON.stringify(request)).not.toContain("rrf");
  });

  it("fills RRF's k with the server default of 60", () => {
    expect(buildSearchRequest({ collection: "kb", fusion: { rrf: {} } }).fusion).toEqual({
      rrf: { k: LOAMS_DEFAULT_RRF_K },
    });
    expect(LOAMS_DEFAULT_RRF_K).toBe(60);
    expect(buildSearchRequest({ collection: "kb", fusion: { rrf: { k: 7 } } }).fusion).toEqual({
      rrf: { k: 7 },
    });
  });

  it("emits the remaining server defaults explicitly", () => {
    expect(buildSearchRequest({ collection: "kb" })).toEqual({
      collection: "kb",
      consistency: "strong",
      retrievers: [],
      sort: [],
      offset: 0,
      limit: 10,
      select: {},
      track_total_hits: "none",
    });
  });

  it("keeps caller values and omits absent optional keys entirely", () => {
    const request = buildSearchRequest({
      collection: "kb",
      limit: 25,
      offset: 5,
      filter: { term: { field: "tenant", value: "a" } },
      score_threshold: 0.4,
      sort: [{ score: { order: "desc" } }],
      search_after: [42],
      track_total_hits: "exact",
      group_by: { field: "tenant" },
    });
    expect(request.limit).toBe(25);
    expect(request.offset).toBe(5);
    expect(request.filter).toEqual({ term: { field: "tenant", value: "a" } });
    expect(request.score_threshold).toBe(0.4);
    expect(request.search_after).toEqual([42]);
    expect(request.track_total_hits).toBe("exact");
    expect(request.group_by).toEqual({ field: "tenant" });
    // Never sent as present-and-null.
    expect(request).not.toHaveProperty("aggregations");
    expect(request).not.toHaveProperty("highlight");
  });

  it("carries an opaque aggregation request through untouched", () => {
    const aggregations = { terms: { field: "tenant", size: 5 } };
    expect(buildSearchRequest({ collection: "kb", aggregations }).aggregations).toEqual(
      aggregations,
    );
  });

  it("POSTs the built body to /query", async () => {
    const { service, fetchMock } = serviceWith({ hits: [], read_token: "v1:s7/p3@1" });
    await service.search({ collection: "kb", retrievers: [TEXT_RETRIEVER] });
    expect(urlOf(fetchMock)).toBe(`${BASE}/v1/namespaces/${NS}/query`);
    const body = bodyOf(fetchMock);
    expect(body["collection"]).toBe("kb");
    expect(body["limit"]).toBe(10);
    expect(body["consistency"]).toBe("strong");
    expect(body["retrievers"]).toEqual([TEXT_RETRIEVER]);
  });
});

describe("/sql is refused unless explicitly opted in", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn<typeof fetch>(async () => reply({ columns: [], rows: [], truncated: false })),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("refuses by default and explains why", async () => {
    const { service, fetchMock } = serviceWith({ columns: [], rows: [], truncated: false });
    await expect(service.sql("SELECT 1")).rejects.toThrow(LoamsApiError);
    await expect(service.sql("SELECT 1")).rejects.toThrow(/allowSql is explicitly true/);
    // And it does not claim to sanitise: SELECT-only enforcement is server-side.
    await expect(service.sql("DROP TABLE kb")).rejects.toThrow(/server-side/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses when allowSql is explicitly false", async () => {
    const { service, fetchMock } = serviceWith({}, { allowSql: false });
    await expect(service.sql("SELECT 1")).rejects.toThrow(/disabled/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("does NOT sanitise the statement when the flag is set — it passes it through verbatim", async () => {
    const { service, fetchMock } = serviceWith(
      { columns: [{ name: "_id", type: "Int64" }], rows: [[1]], truncated: false },
      { allowSql: true },
    );
    const result = await service.sql("SELECT _id FROM kb; -- whatever");
    expect(bodyOf(fetchMock)["query"]).toBe("SELECT _id FROM kb; -- whatever");
    expect(urlOf(fetchMock)).toBe(`${BASE}/v1/namespaces/${NS}/sql`);
    expect(result.columns).toEqual([{ name: "_id", type: "Int64" }]);
    expect(result.rows).toEqual([[1]]);
    expect(result.truncated).toBe(false);
  });

  it("reports a truncated result honestly rather than as a total", async () => {
    const { service } = serviceWith(
      { columns: [], rows: [[1], [2]], truncated: true },
      { allowSql: true },
    );
    expect((await service.sql("SELECT 1")).truncated).toBe(true);
  });

  it("rejects an empty statement", async () => {
    const { service, fetchMock } = serviceWith({}, { allowSql: true });
    await expect(service.sql("   ")).rejects.toThrow(/non-empty/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("is not reachable from any skill", () => {
    expect(loamsLoader.skills?.(runtimeFor(loamsManifest)).map((skill) => skill.id)).not.toContain(
      "sql",
    );
    expect(loamsManifest.agent?.skills.map((skill) => skill.id)).not.toContain("sql");
  });
});

describe("no request ever targets the code-deployment surface", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** Every method a caller can invoke, with arguments that are all valid. */
  function everyCall(service: LoamsAdapterService): Promise<unknown>[] {
    const ids = [1];
    return [
      service.health(),
      service.ready(),
      service.listCollections(),
      service.describeCollection("kb"),
      service.listVersions("kb"),
      service.search({ collection: "kb", retrievers: [TEXT_RETRIEVER] }),
      service.getDocuments("kb", ids),
      service.scrollDocuments("kb", { after: 1 }),
      service.countDocuments("kb", { filter: { match_all: {} } }),
    ];
  }

  it("targets no live or Deploy path, sql opt-in included", async () => {
    for (const allowSql of [false, true]) {
      const fetchMock = vi.fn<typeof fetch>(async () =>
        reply({ columns: [], rows: [], truncated: false, hits: [] }),
      );
      vi.stubGlobal("fetch", fetchMock);
      const service = new LoamsAdapterService(new Context(), config({ allowSql }));

      await Promise.allSettled(everyCall(service));
      await Promise.allSettled([service.sql("SELECT 1")]);

      expect(fetchMock.mock.calls.length).toBeGreaterThan(0);
      for (const call of fetchMock.mock.calls) {
        const url = String(call[0]).toLowerCase();
        expect(url).not.toContain("/live");
        expect(url).not.toContain("deploy");
        // There is no code-execution route in Loams at all; make that explicit.
        expect(url).not.toContain("/exec");
        expect(url).not.toContain("/eval");
        expect(url).not.toContain("/invoke");
        expect(url).not.toContain("/functions");
      }
      vi.unstubAllGlobals();
    }
  });

  it("throws if a path with a live or deploy SEGMENT is ever built", () => {
    const service = new LoamsAdapterService(new Context(), config());
    const build = (service as unknown as { _ns(suffix: string): string })._ns.bind(service);
    expect(() => build("/live/v1/deploy")).toThrow(LoamsApiError);
    expect(() => build("/live/v1/deploy")).toThrow(/never targets it/);
    expect(() => build("/exec")).toThrow(/never targets it/);
  });

  it("does not confuse a collection merely CONTAINING a banned word", () => {
    const service = new LoamsAdapterService(new Context(), config());
    const build = (service as unknown as { _collection(c: string): string })._collection.bind(
      service,
    );
    // "olive" contains "live" and "deployment" contains "deploy"; neither is
    // the code-deployment surface, and refusing them would break a real query.
    expect(build("olive")).toBe(`${LOAMS_API_PREFIX}/namespaces/${NS}/collections/olive`);
    expect(build("deployment-logs")).toBe(
      `${LOAMS_API_PREFIX}/namespaces/${NS}/collections/deployment-logs`,
    );
  });

  it("exposes no Deploy method and no code-execution method", () => {
    const names = Object.getOwnPropertyNames(LoamsAdapterService.prototype);
    for (const forbidden of ["deploy", "exec", "eval", "invoke", "runCode", "functions"]) {
      expect(names.map((n) => n.toLowerCase())).not.toContain(forbidden.toLowerCase());
    }
  });
});

describe("graceful degradation when an optional field is absent", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("treats a missing `collections` array as empty, with a warning", async () => {
    const { service } = serviceWith({});
    await expect(service.listCollections()).resolves.toEqual([]);
  });

  it("treats a `collections` of the wrong type as empty rather than throwing", async () => {
    const { service } = serviceWith({ collections: { nope: true } });
    await expect(service.listCollections()).resolves.toEqual([]);
  });

  it("treats a missing `versions` array as empty", async () => {
    const { service } = serviceWith({});
    await expect(service.listVersions("kb")).resolves.toEqual([]);
  });

  it("treats a missing `hits` array as an empty result", async () => {
    const { service } = serviceWith({ read_token: "v1:s7/p3@1" });
    const result = await service.search({ collection: "kb" });
    expect(result.hits).toEqual([]);
    // Present fields survive the degradation.
    expect(result.read_token).toBe("v1:s7/p3@1");
  });

  it("reports a missing count as 0 rather than NaN", async () => {
    const { service } = serviceWith({ read_token: "v1:s7/p3@1" });
    const result = await service.countDocuments("kb");
    expect(result.count).toBe(0);
    expect(result.read_token).toBe("v1:s7/p3@1");
  });

  it("tolerates a missing read_token everywhere it appears", async () => {
    const { service } = serviceWith({ documents: [] });
    expect((await service.getDocuments("kb", [1])).read_token).toBe("");
    expect((await service.scrollDocuments("kb")).read_token).toBe("");
  });

  it("keeps a collection whose optional hot status is absent", async () => {
    const info = { id: 1, name: "kb", namespace: "demo", live_doc_count: 3 };
    const { service } = serviceWith(info);
    const described = await service.describeCollection("kb");
    expect(described.name).toBe("kb");
    expect(described).not.toHaveProperty("hot");
  });

  it("keeps the owner's hot status when the server does send it", async () => {
    const { service } = serviceWith({
      id: 1,
      name: "kb",
      hot: { vectors: { state: "ready" }, text: { state: "off" }, fragments: { state: "off" } },
    });
    const described = await service.describeCollection("kb");
    expect(described.hot?.vectors.state).toBe("ready");
  });

  it("treats a missing `next` cursor as the end of the collection", async () => {
    const { service } = serviceWith({ documents: [] });
    expect((await service.scrollDocuments("kb")).next).toBeNull();
  });

  it("short-circuits an empty id list without a request", async () => {
    const { service, fetchMock } = serviceWith({ documents: [] });
    expect(await service.getDocuments("kb", [])).toEqual({ documents: [], read_token: "" });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("document reads", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("keeps documents in request order with null for a missing id", async () => {
    const { service } = serviceWith({
      documents: [{ id: 1, source: { body: "a" } }, null, { id: 3 }],
      read_token: "v1:s7/p3@1",
    });
    const result = await service.getDocuments("kb", [1, 2, 3]);
    expect(result.documents).toHaveLength(3);
    expect(result.documents[0]?.id).toBe(1);
    expect(result.documents[1]).toBeNull();
    expect(result.documents[2]?.id).toBe(3);
  });

  it("sends ids and an optional projection, and nothing that was not asked for", async () => {
    const { service, fetchMock } = serviceWith({ documents: [] });
    await service.getDocuments("kb", [1, { uuid: "8-4-4-4-12" }], { select: { source: "none" } });
    const body = bodyOf(fetchMock);
    expect(body["ids"]).toEqual([1, { uuid: "8-4-4-4-12" }]);
    expect(body["select"]).toEqual({ source: "none" });
    expect(body).not.toHaveProperty("consistency");
  });

  it("sends the scroll cursor and the server's default page size", async () => {
    const { service, fetchMock } = serviceWith({ documents: [], next: 50 });
    const result = await service.scrollDocuments("kb", { after: 49 });
    const body = bodyOf(fetchMock);
    expect(body["after"]).toBe(49);
    expect(body["limit"]).toBe(100);
    expect(result.next).toBe(50);
  });

  it("counts the whole collection when no filter is given", async () => {
    const { service, fetchMock } = serviceWith({ count: 128, read_token: "v1:s7/p3@1" });
    const result = await service.countDocuments("kb");
    expect(bodyOf(fetchMock)).toEqual({});
    expect(result.count).toBe(128);
  });

  it("passes a filter through to documents/count", async () => {
    const { service, fetchMock } = serviceWith({ count: 2 });
    await service.countDocuments("kb", { filter: { term: { field: "tenant", value: "a" } } });
    expect(bodyOf(fetchMock)["filter"]).toEqual({ term: { field: "tenant", value: "a" } });
  });
});

describe("UpstreamError surfaces status and body", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads Loams' {error, message} body out of a non-2xx", async () => {
    const { service } = serviceWith(
      { error: "not_found", message: 'namespace "nope" not found' },
      {},
      404,
    );
    const err = await service.listCollections().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LoamsApiError);
    const apiError = err as LoamsApiError;
    expect(apiError.status).toBe(404);
    expect(apiError.code).toBe("not_found");
    expect(apiError.message).toBe('namespace "nope" not found');
  });

  it("surfaces the extra fields a Loams error attaches", async () => {
    const { service } = serviceWith(
      { error: "invalid_argument", message: "bad op 2", index: 2 },
      {},
      400,
    );
    const err = (await service.countDocuments("kb").catch((e: unknown) => e)) as LoamsApiError;
    expect(err.details["index"]).toBe(2);
    expect(err).not.toHaveProperty("details.error");
    expect(err).not.toHaveProperty("details.message");
  });

  it("falls back to the raw body when the error is not a Loams envelope", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn<typeof fetch>(
        async () => new Response("<html>502 Bad Gateway</html>", { status: 502 }),
      ),
    );
    const service = new LoamsAdapterService(new Context(), config());
    const err = (await service.listCollections().catch((e: unknown) => e)) as LoamsApiError;
    expect(err).toBeInstanceOf(LoamsApiError);
    expect(err.status).toBe(502);
    expect(err.code).toBeUndefined();
    expect(err.message).toMatch(/502/);
  });

  it("reports liveness as false rather than throwing on a failing probe", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn<typeof fetch>(async () => new Response("", { status: 503 })),
    );
    const service = new LoamsAdapterService(new Context(), config());
    await expect(service.health()).resolves.toBe(false);
    await expect(service.ready()).resolves.toBe(false);
  });

  it("reports liveness as true when both probes answer 2xx", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn<typeof fetch>(async () => new Response("", { status: 200 })),
    );
    const service = new LoamsAdapterService(new Context(), config());
    await expect(service.health()).resolves.toBe(true);
    await expect(service.ready()).resolves.toBe(true);
  });
});

describe("optional bearer token", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** The `Authorization` header the nth fetch was issued with. */
  function authOf(fetchMock: MockedFetch, index = 0): string | null {
    const init = fetchMock.mock.calls[index]?.[1] as RequestInit | undefined;
    return new Headers(init?.headers).get("Authorization");
  }

  it("sends no Authorization header when no token is configured", async () => {
    const { service, fetchMock } = serviceWith({ collections: [] });
    await service.listCollections();
    expect(authOf(fetchMock)).toBeNull();
  });

  it("sends a bearer token when one is configured, for a proxy-fronted deployment", async () => {
    const { service, fetchMock } = serviceWith({ collections: [] }, { token: "s3cr3t" });
    await service.listCollections();
    expect(authOf(fetchMock)).toBe("Bearer s3cr3t");
  });
});

describe("loams manifest", () => {
  it("declares the manifest contract", () => {
    expect(loamsManifest.id).toBe("loams");
    expect(loamsManifest.uiPath).toBe("/plugins/loams");
    expect(loamsManifest.upstream).toEqual({ product: "Loams", envPrefix: "LOAMS" });
    expect(loamsManifest.category).toBeTruthy();
    expect(loamsManifest.version).toBeTruthy();
    expect(loamsManifest.description?.length).toBeGreaterThan(0);
  });

  it("carries the maturity caveat in its description", () => {
    expect(loamsManifest.description).toMatch(/no stable release/i);
    expect(loamsManifest.description).toMatch(/degrade/i);
  });

  it("sorts outside the band the other adapters occupy (30-36)", () => {
    expect(loamsManifest.order).toBeGreaterThan(36);
  });

  it("pairs every declared skill with a loader handler", () => {
    const declared = (loamsManifest.agent?.skills ?? []).map((skill) => skill.id);
    const handled = (loamsLoader.skills?.(runtimeFor(loamsManifest)) ?? []).map(
      (skill) => skill.id,
    );
    expect(declared.length).toBeGreaterThan(0);
    expect([...handled].sort()).toEqual([...declared].sort());
  });

  it("has a well-formed description per skill", () => {
    for (const skill of loamsManifest.agent?.skills ?? []) {
      expect(skill.id).toMatch(/^[a-zA-Z][a-zA-Z0-9]*$/);
      expect(skill.description.length).toBeGreaterThan(0);
      expect(skill.name.length).toBeGreaterThan(0);
    }
  });

  it("points every skill at a method the service actually implements", () => {
    const methods = new Set(Object.getOwnPropertyNames(LoamsAdapterService.prototype));
    const bySkill: Record<string, keyof LoamsAdapterService> = {
      listCollections: "listCollections",
      describeCollection: "describeCollection",
      listVersions: "listVersions",
      search: "search",
      getDocuments: "getDocuments",
      scrollDocuments: "scrollDocuments",
      countDocuments: "countDocuments",
    };
    for (const skill of loamsManifest.agent?.skills ?? []) {
      if (skill.id === "getStatus") continue; // composed from health() + ready()
      expect(methods.has(bySkill[skill.id] as string)).toBe(true);
    }
  });
});
