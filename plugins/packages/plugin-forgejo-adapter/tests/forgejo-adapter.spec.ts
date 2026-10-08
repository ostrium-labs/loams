import { describe, it, expect, vi, beforeEach, afterEach } from "vite-plus/test";
import { Context } from "cordis";
import { AgentBus, HttpRouter, type PluginManifest, type PluginRuntime } from "@loams-core/host";
import { UpstreamError } from "@loams-core/http";
import {
  ForgejoAdapterService,
  clampLimit,
  forgejoLoader,
  forgejoManifest,
  isNodeinfoUsageTrustworthy,
  parseLinkHeader,
  parseTotalCount,
} from "../src/service.js";
import type { ForgejoConfig, ForgejoIssue, ForgejoRepository } from "../src/types.js";

/**
 * `PluginLoader.skills` is handed a `PluginRuntime`. These loaders build their
 * handler list from a literal and never read the runtime, but the contract
 * requires one, so give them a real (empty) one rather than casting `undefined`.
 */
function runtimeFor(manifest: PluginManifest): PluginRuntime {
  const ctx = new Context();
  return { id: manifest.id, manifest, ctx, router: new HttpRouter(ctx), bus: new AgentBus(ctx) };
}

const BASE = "https://git.example.test";

type MockedFetch = ReturnType<typeof vi.fn<typeof fetch>>;

function reply(body: unknown, headers: Record<string, string> = {}, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });
}

function urlOf(fetchMock: MockedFetch, index = 0): string {
  return String(fetchMock.mock.calls[index]?.[0]);
}

function repo(overrides: Partial<ForgejoRepository> = {}): ForgejoRepository {
  return {
    id: 9,
    name: "widgets",
    full_name: "acme/widgets",
    owner: { id: 1, login: "acme", active: true, created: "2020-01-01T00:00:00Z" },
    size: 4096,
    stars_count: 128,
    forks_count: 12,
    watchers_count: 130,
    open_issues_count: 4,
    open_pr_counter: 2,
    release_counter: 7,
    created_at: "2020-01-01T00:00:00Z",
    updated_at: "2024-05-01T00:00:00Z",
    ...overrides,
  };
}

function issue(overrides: Partial<ForgejoIssue> = {}): ForgejoIssue {
  return {
    id: 100,
    number: 42,
    title: "Widget explodes",
    state: "open",
    user: { id: 1, login: "ada", active: true, created: "2020-01-01T00:00:00Z" },
    comments: 3,
    created_at: "2024-01-01T00:00:00Z",
    updated_at: "2024-02-01T00:00:00Z",
    due_date: "2024-06-01T00:00:00Z",
    assets: [{ id: 1 }],
    pull_request: null,
    ...overrides,
  };
}

describe("ForgejoAdapterService", () => {
  let fetchMock: MockedFetch;
  const built: ForgejoAdapterService[] = [];

  function makeService(overrides: Partial<ForgejoConfig> = {}): ForgejoAdapterService {
    const created = new ForgejoAdapterService(new Context(), {
      baseUrl: BASE,
      token: "PAT123",
      ...overrides,
    });
    built.push(created);
    return created;
  }

  let service: ForgejoAdapterService;

  beforeEach(() => {
    const stub = vi.fn();
    vi.stubGlobal("fetch", stub);
    fetchMock = stub as unknown as MockedFetch;
    service = makeService();
  });

  afterEach(() => {
    for (const created of built) created.detach();
    built.length = 0;
    vi.unstubAllGlobals();
  });

  /* ------------------------------------------------------------------ */
  /* Auth                                                                */
  /* ------------------------------------------------------------------ */

  it("sends exactly `token <pat>` as the Authorization header", async () => {
    fetchMock.mockResolvedValueOnce(reply({ version: "9.0.2" }));

    await service.getVersion();

    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect(new Headers(init.headers).get("Authorization")).toBe("token PAT123");
  });

  it("never puts the token in the query string", async () => {
    fetchMock.mockResolvedValueOnce(reply([repo()]));
    await service.listOrganizations();

    const url = urlOf(fetchMock);
    expect(url).not.toContain("token=");
    expect(url).not.toContain("access_token=");
    expect(url).not.toContain("PAT123");
  });

  it("uses /api/v1, not the near-empty /api/forgejo/v1", async () => {
    fetchMock.mockResolvedValueOnce(reply({ version: "9.0.2" }));
    await service.getVersion();
    expect(new URL(urlOf(fetchMock)).pathname).toBe("/api/v1/version");
  });

  /* ------------------------------------------------------------------ */
  /* No envelope                                                         */
  /* ------------------------------------------------------------------ */

  it("parses a list endpoint that returns a BARE JSON array", async () => {
    fetchMock.mockResolvedValueOnce(reply([repo(), repo({ id: 10 })]));

    const page = await service.listOrganizations();

    expect(page.items).toHaveLength(2);
    expect(page.items[0].full_name).toBe("acme/widgets");
  });

  it("rejects a wrapped {items:[...]} list, naming the bare-array shape", async () => {
    fetchMock.mockResolvedValueOnce(reply({ items: [repo()] }));
    await expect(service.listOrganizations()).rejects.toThrow(/bare JSON array/);
  });

  it("parses /repos/search as {ok, data} rather than a bare array", async () => {
    fetchMock.mockResolvedValueOnce(reply({ ok: true, data: [repo()] }));

    const result = await service.searchRepositories("widgets");

    expect(result.ok).toBe(true);
    expect(result.data).toHaveLength(1);
    expect(new URL(urlOf(fetchMock)).pathname).toBe("/api/v1/repos/search");
  });

  it("surfaces ok:false from /repos/search rather than assuming success", async () => {
    fetchMock.mockResolvedValueOnce(reply({ ok: false, data: [] }));
    const result = await service.searchRepositories("nope");
    expect(result.ok).toBe(false);
  });

  /* ------------------------------------------------------------------ */
  /* Pagination headers                                                  */
  /* ------------------------------------------------------------------ */

  it("parses X-Total-Count, which is a decimal STRING", async () => {
    fetchMock.mockResolvedValueOnce(reply([repo()], { "X-Total-Count": "1234" }));

    const page = await service.listOrganizations();

    expect(page.totalCount).toBe(1234);
  });

  it("reports the total as UNDEFINED when X-Total-Count is absent, not zero", async () => {
    fetchMock.mockResolvedValueOnce(reply([repo()]));

    const page = await service.listOrganizations();

    expect(page.totalCount).toBeUndefined();
  });

  it("parses the RFC 5988 Link header into rels and page numbers", async () => {
    fetchMock.mockResolvedValueOnce(
      reply([repo()], {
        Link: '<https://git.example.test/api/v1/orgs?page=2>; rel="next", <https://git.example.test/api/v1/orgs?page=9>; rel="last"',
      }),
    );

    const page = await service.listOrganizations();

    expect(page.link?.nextPage).toBe(2);
    expect(page.link?.lastPage).toBe(9);
    expect(page.link?.next).toContain("page=2");
  });

  it("exposes the header parsers directly, including their absence cases", () => {
    expect(parseTotalCount(new Headers({ "X-Total-Count": " 42 " }))).toBe(42);
    expect(parseTotalCount(new Headers())).toBeUndefined();
    expect(parseTotalCount(new Headers({ "X-Total-Count": "lots" }))).toBeUndefined();
    expect(parseLinkHeader(new Headers())).toBeUndefined();
  });

  it("reads the headers off its own response, never a queued one from an earlier call", async () => {
    // The regression this migration exists for. Headers used to be queued per URL
    // by a module-scoped `fetch` wrapper, and a response that arrived and then FAILED
    // was never claimed — so it stayed in the queue and the next successful call to
    // the same URL reported the failed request's total.
    fetchMock
      .mockResolvedValueOnce(reply({ message: "boom" }, { "X-Total-Count": "999" }, 500))
      .mockResolvedValueOnce(reply([issue()], { "X-Total-Count": "7" }));

    await expect(service.listIssues("acme", "widgets")).rejects.toBeInstanceOf(UpstreamError);
    const page = await service.listIssues("acme", "widgets");

    expect(page.totalCount).toBe(7);
    expect(page.totalCount).not.toBe(999);
  });

  it("leaves globalThis.fetch alone — the headers come from the client's own observer", async () => {
    // The wrapper this adapter used to install at module scope is gone.
    const before = globalThis.fetch;
    const extra = makeService();

    fetchMock.mockImplementation(async () => reply([repo()], { "X-Total-Count": "3" }));
    await service.listOrganizations();
    await extra.listOrganizations();

    expect(globalThis.fetch).toBe(before);
    expect(globalThis.fetch).toBe(fetchMock);
  });

  /* ------------------------------------------------------------------ */
  /* The 50-item clamp                                                   */
  /* ------------------------------------------------------------------ */

  it("clamps limit to 50 locally instead of asking for 1000 and being silently cut", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.listOrganizations({ limit: 1000 });

    const limit = new URL(urlOf(fetchMock)).searchParams.get("limit");
    expect(limit).toBe("50");
    // The clamp is not fought: no error, no retry, and a normal page result.
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("clamps in the helper too, and never exceeds the server cap", () => {
    expect(clampLimit(1000)).toBe(50);
    expect(clampLimit(200, 10)).toBe(10);
    expect(clampLimit(25)).toBe(25);
    expect(clampLimit(undefined)).toBe(30);
    expect(clampLimit(1000, 200)).toBe(50);
    expect(clampLimit(0)).toBe(1);
  });

  /* ------------------------------------------------------------------ */
  /* Field names                                                         */
  /* ------------------------------------------------------------------ */

  it("reads repository counters under their real names, not the guessed ones", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        id: 9,
        name: "widgets",
        full_name: "acme/widgets",
        owner: { id: 1, login: "acme", active: true, created: "2020-01-01T00:00:00Z" },
        size: 4096,
        stars_count: 128,
        forks_count: 12,
        watchers_count: 130,
        open_issues_count: 4,
        open_pr_counter: 2,
        release_counter: 7,
        created_at: "2020-01-01T00:00:00Z",
        updated_at: "2024-05-01T00:00:00Z",
      }),
    );

    const found = await service.getRepository("acme", "widgets");

    expect(found.stars_count).toBe(128);
    expect(found.forks_count).toBe(12);
    expect(found.watchers_count).toBe(130);
    expect(found.open_issues_count).toBe(4);
    // `_counter`, not `_count`; and singular `release`.
    expect(found.open_pr_counter).toBe(2);
    expect(found.release_counter).toBe(7);
    // The names the API does NOT use are absent.
    expect(found).not.toHaveProperty("stars");
    expect(found).not.toHaveProperty("open_pr_count");
    expect(found).not.toHaveProperty("releases_counter");
    expect(found.owner.login).toBe("acme");
    expect(found.size).toBe(4096);
  });

  it("reads a user as login / active / created, not username / is_active / created_at", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        id: 3,
        login: "ada",
        full_name: "Ada Lovelace",
        email: "",
        avatar_url: "",
        active: true,
        is_admin: false,
        restricted: false,
        created: "2021-06-01T10:00:00Z",
      }),
    );

    const user = await service.getUser("ada");

    expect(user.login).toBe("ada");
    expect(user.active).toBe(true);
    expect(user.created).toBe("2021-06-01T10:00:00Z");
    expect(user).not.toHaveProperty("username");
    expect(user).not.toHaveProperty("is_active");
    expect(user).not.toHaveProperty("created_at");
    expect(new URL(urlOf(fetchMock)).pathname).toBe("/api/v1/users/ada");
  });

  it("reads an issue as number / user / assets / due_date, with comments as a COUNT", async () => {
    fetchMock.mockResolvedValueOnce(reply([issue()]));

    const page = await service.listIssues("acme", "widgets");

    const row = page.items[0];
    expect(row.number).toBe(42);
    expect(row.user.login).toBe("ada");
    expect(row.assets).toEqual([{ id: 1 }]);
    expect(row.due_date).toBe("2024-06-01T00:00:00Z");
    expect(row.comments).toBe(3);
    expect(row).not.toHaveProperty("index");
    expect(row).not.toHaveProperty("poster");
    expect(row).not.toHaveProperty("attachments");
    expect(row).not.toHaveProperty("deadline");
  });

  it("keeps merged distinct from state and reads branch names from `label`", async () => {
    fetchMock.mockResolvedValueOnce(
      reply([
        {
          id: 5,
          number: 9,
          title: "Add thing",
          state: "closed",
          merged: true,
          merged_at: "2024-03-01T00:00:00Z",
          base: { label: "main", ref: "refs/heads/main", sha: "aaa", repo_id: 9 },
          head: { label: "feature", ref: "refs/heads/feature", sha: "bbb", repo_id: 9 },
          user: { id: 3, login: "ada", active: true, created: "2021-06-01T10:00:00Z" },
          created_at: "2024-02-25T00:00:00Z",
          updated_at: "2024-03-01T00:00:00Z",
          changed_files: 12,
          merge_commit_sha: "ccc",
        },
      ]),
    );

    const page = await service.listPullRequests("acme", "widgets");

    const pr = page.items[0];
    expect(pr.state).toBe("closed");
    expect(pr.merged).toBe(true);
    expect(pr.base.label).toBe("main");
    expect(pr.head.label).toBe("feature");
    expect(pr.base).not.toHaveProperty("name");
    expect(pr.changed_files).toBe(12);
  });

  /* ------------------------------------------------------------------ */
  /* type=issues                                                         */
  /* ------------------------------------------------------------------ */

  it("always sends type=issues on the issues listing so pull requests are excluded", async () => {
    fetchMock.mockResolvedValueOnce(reply([issue()]));

    await service.listIssues("acme", "widgets");

    const url = new URL(urlOf(fetchMock));
    expect(url.searchParams.get("type")).toBe("issues");
    // state is explicit too: the server default is `open`, not `all`.
    expect(url.searchParams.get("state")).toBe("open");
  });

  it("keeps type=issues even when a caller asks for every state", async () => {
    fetchMock.mockResolvedValueOnce(reply([issue()]));
    await service.listIssues("acme", "widgets", { state: "all", since: "2024-05-01T00:00:00Z" });

    const url = new URL(urlOf(fetchMock));
    expect(url.searchParams.get("type")).toBe("issues");
    expect(url.searchParams.get("state")).toBe("all");
    // `since` is the incremental-sync primitive.
    expect(url.searchParams.get("since")).toBe("2024-05-01T00:00:00Z");
  });

  it("sends type=issues on the cross-repository issue search too", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));
    await service.searchIssues("widget");

    const url = new URL(urlOf(fetchMock));
    expect(url.pathname).toBe("/api/v1/repos/issues/search");
    expect(url.searchParams.get("type")).toBe("issues");
  });

  it("derives an incremental watermark from max(updated_at)", async () => {
    fetchMock.mockResolvedValueOnce(
      reply([
        issue({ updated_at: "2024-02-01T00:00:00Z" }),
        issue({ updated_at: "2024-09-09T00:00:00Z" }),
      ]),
    );
    const page = await service.listIssuesUpdatedSince("acme", "widgets", "2024-01-01T00:00:00Z");

    expect(service.maxUpdatedAt(page.items)).toBe("2024-09-09T00:00:00Z");
    expect(service.maxUpdatedAt([])).toBeUndefined();
  });

  /* ------------------------------------------------------------------ */
  /* Commits                                                             */
  /* ------------------------------------------------------------------ */

  it("always sends stat=false, files=false and verification=false", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    await service.listCommits("acme", "widgets");

    const url = new URL(urlOf(fetchMock));
    expect(url.searchParams.get("stat")).toBe("false");
    expect(url.searchParams.get("files")).toBe("false");
    expect(url.searchParams.get("verification")).toBe("false");
  });

  it("keeps the commit flags off even when paging a large history", async () => {
    fetchMock.mockImplementation(async () =>
      reply(
        Array.from({ length: 50 }, () => ({
          sha: "a".repeat(40),
          author: null,
          committer: null,
          commit: {
            author: { name: "A", email: "a@example.test", date: "2024-01-01T00:00:00Z" },
            committer: { name: "A", email: "a@example.test", date: "2024-01-01T00:00:00Z" },
            message: "x",
          },
          parents: [],
        })),
      ),
    );

    await service.listCommits("acme", "widgets", { page: 3, sha: "main", not: "main~1..main" });

    const url = new URL(urlOf(fetchMock));
    expect(url.searchParams.get("stat")).toBe("false");
    expect(url.searchParams.get("files")).toBe("false");
    expect(url.searchParams.get("verification")).toBe("false");
    expect(url.searchParams.get("sha")).toBe("main");
    expect(url.searchParams.get("not")).toBe("main~1..main");
    expect(url.searchParams.get("page")).toBe("3");
  });

  it("handles an EMPTY repository as 409, not as an empty list", async () => {
    fetchMock.mockResolvedValueOnce(reply({ message: "Git Repository is empty." }, {}, 409));

    const result = await service.listCommits("acme", "empty");

    expect(result.empty).toBe(true);
    expect(result.commits).toEqual([]);
    // It is NOT the same as a repository with no visible history.
    expect(result).not.toHaveProperty("totalCount", 0);
  });

  it("derives contributors from all three author representations, not just author.login", async () => {
    fetchMock.mockImplementation(async () =>
      reply([
        {
          // Registered: top-level author resolved by email.
          sha: "s1",
          author: {
            id: 1,
            login: "ada",
            active: true,
            created: "2021-01-01T00:00:00Z",
            full_name: "Ada",
          },
          // ...and a DIFFERENT committer for the same commit.
          committer: { id: 2, login: "bob", active: true, created: "2021-01-01T00:00:00Z" },
          commit: {
            author: { name: "Ada", email: "ada@example.test", date: "2024-01-01T00:00:00Z" },
            committer: { name: "Bob", email: "bob@example.test", date: "2024-01-01T00:00:00Z" },
            message: "one",
          },
          parents: [],
        },
        {
          // Unregistered: top-level author is null but the git signature exists.
          sha: "s2",
          author: null,
          committer: null,
          commit: {
            author: { name: "Someone", email: "ghost@example.test", date: "2024-01-02T00:00:00Z" },
            committer: {
              name: "Someone",
              email: "ghost@example.test",
              date: "2024-01-02T00:00:00Z",
            },
            message: "two",
          },
          parents: [],
        },
      ]),
    );

    const contributors = await service.aggregateContributors("acme", "widgets", { maxPages: 1 });

    expect(contributors.map((c) => [c.login, c.commits])).toEqual([
      ["ada", 1],
      [null, 1],
    ]);
    const ghost = contributors.find((c) => c.login === null);
    expect(ghost?.unregistered).toBe(true);
    expect(ghost?.email).toBe("ghost@example.test");
    // The committer is deliberately not counted: `author` and `committer` are
    // resolved independently, and only the author is a contribution.
    expect(contributors.some((c) => c.login === "bob")).toBe(false);
  });

  it("never calls a contributors endpoint, because none exists", async () => {
    fetchMock.mockImplementation(async () => reply([]));
    await service.aggregateContributors("acme", "widgets", { maxPages: 1 });
    for (const call of fetchMock.mock.calls) {
      expect(String(call[0])).not.toMatch(/contributors|contributors\/stats/);
    }
  });

  /* ------------------------------------------------------------------ */
  /* Silent-zero traps                                                   */
  /* ------------------------------------------------------------------ */

  it("detects a heatmap-disabled instance by its 404", async () => {
    fetchMock.mockResolvedValueOnce(reply({ message: "Not Found", url: "…" }, {}, 404));

    const result = await service.getUserHeatmap("ada");

    expect(result.supported).toBe(false);
    expect(result.entries).toEqual([]);
    // An empty array that means "cannot tell" must never be drawn as a real zero.
    expect(result.ambiguous).toBe(true);
  });

  it("reports an empty but present heatmap as ambiguous, since [] means no access OR no activity", async () => {
    fetchMock.mockResolvedValueOnce(reply([]));

    const result = await service.getUserHeatmap("quiet");

    expect(result.supported).toBe(true);
    expect(result.entries).toEqual([]);
    expect(result.ambiguous).toBe(true);
  });

  it("reports a populated heatmap as unambiguous", async () => {
    fetchMock.mockResolvedValueOnce(reply([{ timestamp: 1_700_000_000, contributions: 3 }]));

    const result = await service.getUserHeatmap("busy");

    expect(result.supported).toBe(true);
    expect(result.ambiguous).toBe(false);
    expect(result.entries[0].contributions).toBe(3);
  });

  it("detects nodeinfo usage that is all zeros because statistics are not shared", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        version: "9.0.2",
        software: { name: "forgejo", version: "9.0.2" },
        protocols: ["activitypub"],
        usage: {
          users: { activeMonth: 0, activeHalfyear: 0, total: 0 },
          localPosts: 0,
          localComments: 0,
        },
      }),
    );

    const result = await service.getNodeInfo();

    expect(result.available).toBe(true);
    expect(result.usageTrustworthy).toBe(false);
    expect(result.caveats.join(" ")).toMatch(/SHARE_USER_STATISTICS/);
    expect(result.caveats.join(" ")).toMatch(/180s/);
  });

  it("accepts nodeinfo usage when the counters are real", async () => {
    fetchMock.mockResolvedValueOnce(
      reply({
        version: "9.0.2",
        software: { name: "forgejo", version: "9.0.2" },
        protocols: [],
        usage: {
          users: { activeMonth: 4, activeHalfyear: 9, total: 40 },
          localPosts: 12,
          localComments: 3,
        },
      }),
    );

    const result = await service.getNodeInfo();

    expect(result.usageTrustworthy).toBe(true);
    expect(isNodeinfoUsageTrustworthy(result.nodeinfo)).toBe(true);
    expect(isNodeinfoUsageTrustworthy(undefined)).toBe(false);
  });

  it("reports nodeinfo disabled when the instance answers 404", async () => {
    fetchMock.mockResolvedValueOnce(reply({ message: "Not Found", url: "…" }, {}, 404));
    const result = await service.getNodeInfo();
    expect(result.available).toBe(false);
    expect(result.usageTrustworthy).toBe(false);
  });

  /* ------------------------------------------------------------------ */
  /* Self-imposed limits                                                 */
  /* ------------------------------------------------------------------ */

  it("builds no rate-limit retry logic, because Forgejo has no rate limiting", async () => {
    const src = Object.getOwnPropertyNames(ForgejoAdapterService.prototype).join(" ");
    expect(src).not.toMatch(/rateLimit|RateLimit|retry|backoff/i);
  });

  it("limits its own concurrency and caches within one token's service", async () => {
    const cached = makeService();
    fetchMock.mockImplementation(async () => reply([repo()], { "X-Total-Count": "1" }));

    await cached.listOrganizations();
    await cached.listOrganizations();

    // Second call served from the per-instance cache.
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("caches /orgs per instance, so one token's org list cannot reach another token", async () => {
    const limited = makeService();
    fetchMock.mockImplementation(async () => reply([repo()]));
    await limited.listOrganizations();

    const other = makeService();
    await other.listOrganizations();

    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});

describe("forgejo manifest", () => {
  it("declares the manifest contract", () => {
    expect(forgejoManifest.id).toBe("forgejo");
    expect(forgejoManifest.uiPath).toBe("/plugins/forgejo");
    expect(forgejoManifest.upstream).toEqual({ product: "Forgejo", envPrefix: "FORGEJO" });
    expect(forgejoManifest.agent?.skills.length).toBeGreaterThan(0);
  });

  it("pairs every declared skill with a loader handler", () => {
    const declared = (forgejoManifest.agent?.skills ?? []).map((skill) => skill.id);
    const handled = (forgejoLoader.skills?.(runtimeFor(forgejoManifest)) ?? []).map(
      (skill) => skill.id,
    );
    expect(handled).toEqual(declared);
  });
});
