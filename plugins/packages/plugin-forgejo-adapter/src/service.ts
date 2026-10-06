/**
 * Forgejo adapter.
 *
 * SCOPE: read-only. Every method here issues GET. That is not a style choice:
 * Forgejo derives the required scope from the HTTP method, so a GET only ever
 * needs a `read:*` scope and a POST would need a `write:*` one this adapter has
 * no business holding.
 *
 * WHAT IS REUSED
 * --------------
 * All HTTP mechanics come from `@loams-plugins/plugin-upstream-http`. What lives here is
 * what is genuinely Forgejo's: the absence of an envelope, bare-array list
 * responses, `X-Total-Count` / `Link` pagination, and the set of responses that
 * return success while meaning something other than what they look like.
 *
 * THREE THINGS WORTH READING BEFORE EDITING
 * -----------------------------------------
 * 1. `/api/v1`, not `/api/forgejo/v1`. The latter exists in 9.0.2 and contains a
 *    root and a version route and almost nothing else.
 * 2. There is NO rate limiting. No 429, no `X-RateLimit-*`, ever. So there is no
 *    retry logic here, and the actual protection is a local concurrency limit
 *    plus a short cache — the adapter limits itself.
 * 3. Missing auth is frequently 403, not 401. Anything keying retry on 401 alone
 *    will miss it. The shared client already treats 401 and 403 as auth failures.
 */

import { Context, Service } from "cordis";
import {
  QueryValue,
  UpstreamClient,
  UpstreamError,
  UpstreamLogger,
  loggerFrom,
} from "@loams-plugins/plugin-upstream-http";
import type { PluginAgentSkill, PluginLoader, PluginManifest } from "@loams-plugins/core";
import {
  FORGEJO_DEFAULT_LIMIT,
  FORGEJO_MAX_LIMIT,
  ForgejoActivity,
  ForgejoBranch,
  ForgejoCommit,
  ForgejoCommitPage,
  ForgejoConfig,
  ForgejoContributor,
  ForgejoHeatmapResult,
  ForgejoIssue,
  ForgejoLink,
  ForgejoNodeInfo,
  ForgejoNodeInfoResult,
  ForgejoOrganization,
  ForgejoPage,
  ForgejoPullRequest,
  ForgejoRepository,
  ForgejoRepositorySearch,
  ForgejoTag,
  ForgejoTeam,
  ForgejoUser,
  ForgejoVersion,
} from "./types.js";

/**
 * The API prefix.
 *
 * `/api/forgejo/v1` also exists in 9.0.2 but carries only a root and a version
 * route, and is not where these endpoints live.
 */
export const FORGEJO_API_PREFIX = "/api/v1";

const DEFAULT_CONCURRENCY = 4;
const DEFAULT_CACHE_TTL_MS = 60_000;

/* -------------------------------------------------------------------------- */
/* Pagination header parsing                                                   */
/* -------------------------------------------------------------------------- */

/**
 * Parse `X-Total-Count`.
 *
 * It is a DECIMAL STRING, not a number, and it may be absent entirely — in which
 * case the total is genuinely unknown, not zero.
 */
export function parseTotalCount(headers: Headers | undefined): number | undefined {
  const raw = headers?.get?.("X-Total-Count");
  if (typeof raw !== "string") return undefined;
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) return undefined;
  const parsed = Number(trimmed);
  return Number.isSafeInteger(parsed) ? parsed : undefined;
}

/** Parse an RFC 5988 `Link` header into its rels and the page numbers they imply. */
export function parseLinkHeader(headers: Headers | undefined): ForgejoLink | undefined {
  const raw = headers?.get?.("Link");
  if (typeof raw !== "string" || raw.trim().length === 0) return undefined;

  const pageOf = (url: string): number | undefined => {
    const match = /[?&]page=(\d+)/.exec(url);
    return match ? Number(match[1]) : undefined;
  };

  const link: ForgejoLink = {};
  for (const part of raw.split(",")) {
    const match = /^<([^>]+)>\s*;\s*rel="([^"]+)"/.exec(part.trim());
    if (!match) continue;
    const [, url, rel] = match;
    switch (rel) {
      case "next":
        link.next = url;
        link.nextPage = pageOf(url);
        break;
      case "prev":
        link.prev = url;
        link.prevPage = pageOf(url);
        break;
      case "first":
        link.first = url;
        break;
      case "last":
        link.last = url;
        link.lastPage = pageOf(url);
        break;
      default:
        break;
    }
  }
  return Object.keys(link).length > 0 ? link : undefined;
}

/**
 * Forgejo clamps `limit` to 50 SILENTLY: `limit=1000` returns 50 items with no
 * error and nothing in the response says so. Rather than discover that from a
 * short page, the cap is applied here too — so what is requested is what is
 * expected back, and a caller paginating by 1000 does not believe it is getting
 * 1000.
 */
export function clampLimit(requested: number | undefined, configured?: number): number {
  const ceiling = limitCeiling(configured);
  const wanted = requested ?? FORGEJO_DEFAULT_LIMIT;
  if (!Number.isFinite(wanted)) return Math.min(FORGEJO_DEFAULT_LIMIT, ceiling);
  return Math.max(1, Math.min(Math.trunc(wanted), ceiling));
}

/**
 * The largest `limit` this adapter will ever ask for.
 *
 * This is a CEILING, not a default: it only comes into play when a caller passes
 * an explicit `limit` above it, or when the configured ceiling is below
 * Forgejo's own 50.
 */
export function limitCeiling(configured?: number): number {
  if (configured === undefined || !Number.isFinite(configured)) return FORGEJO_MAX_LIMIT;
  return Math.max(1, Math.min(Math.trunc(configured), FORGEJO_MAX_LIMIT));
}

/**
 * Sanity-check nodeinfo's usage block.
 *
 * Every counter is zero unless the instance enables
 * `[federation] SHARE_USER_STATISTICS`. All-zero therefore means "not shared",
 * not "nobody is here", and a chart drawn from it is a chart of nothing.
 */
export function isNodeinfoUsageTrustworthy(nodeinfo: ForgejoNodeInfo | undefined): boolean {
  const usage = nodeinfo?.usage;
  if (!usage) return false;
  const { activeMonth, activeHalfyear, total } = usage.users ?? {};
  return (
    (activeMonth ?? 0) > 0 ||
    (activeHalfyear ?? 0) > 0 ||
    (total ?? 0) > 0 ||
    (usage.localPosts ?? 0) > 0 ||
    (usage.localComments ?? 0) > 0
  );
}

function requireArray<T>(value: unknown, what: string): T[] {
  if (!Array.isArray(value)) {
    throw new Error(
      `${what}: expected a bare JSON array, got ${value === undefined ? "undefined" : typeof value}` +
        " — Forgejo list endpoints do not wrap their results",
    );
  }
  return value as T[];
}

function repoPath(owner: string, repo: string): string {
  return `/repos/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}`;
}

/* -------------------------------------------------------------------------- */
/* Service                                                                     */
/* -------------------------------------------------------------------------- */

export class ForgejoAdapterService extends Service {
  static inject = [];

  readonly config: ForgejoConfig;
  private readonly client: UpstreamClient;
  private readonly log: UpstreamLogger | undefined;
  private readonly maxConcurrency: number;
  private readonly cacheTtlMs: number;
  private readonly maxLimit: number;
  /** Per-token response cache. See {@link ForgejoConfig.cacheTtlMs}. */
  private readonly cache = new Map<string, { expiresAt: number; value: unknown }>();
  private inFlight = 0;
  private readonly waiting: Array<() => void> = [];

  constructor(ctx: Context, config: ForgejoConfig) {
    super(ctx, "forgejo");
    this.config = config;
    this.log = loggerFrom(ctx);
    this.maxConcurrency = config.concurrency ?? DEFAULT_CONCURRENCY;
    this.cacheTtlMs = config.cacheTtlMs ?? DEFAULT_CACHE_TTL_MS;
    this.maxLimit = limitCeiling(config.limit);

    // `token <pat>` is correct and is what services/auth/method/util.go accepts
    // (it also accepts `bearer`, case-insensitively, with exactly two fields).
    // App passwords would use HTTP Basic instead, and Basic plus 2FA would then
    // need an X-Forgejo-OTP header on every request. A PAT avoids both.
    this.client = new UpstreamClient(
      {
        baseUrl: config.baseUrl.replace(/\/+$/, ""),
        auth: { kind: "authorization-raw", value: `token ${config.token}` },
        timeoutMs: config.timeoutMs,
      },
      this.log,
    );
  }

  /**
   * Release this instance's response cache.
   *
   * Cordis's `Service` has no teardown hook to override, so this is the adapter's
   * own disposer; call it when the plugin is unloaded. Nothing else is held: the
   * pagination headers come from the shared client's per-request response
   * observer rather than from a module-scoped `fetch` wrapper, so there is
   * nothing global left to unregister.
   */
  detach(): void {
    this.cache.clear();
  }

  /* ---------------------------------------------------------------------- */
  /* Plumbing                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * Bound concurrency.
   *
   * Forgejo does not rate limit, so this is the only brake on the adapter. The
   * real failure mode being prevented is self-inflicted: a dashboard fanning out
   * over hundreds of repositories would otherwise open hundreds of sockets.
   */
  private async _withSlot<T>(work: () => Promise<T>): Promise<T> {
    if (this.inFlight >= this.maxConcurrency) {
      await new Promise<void>((resolve) => {
        this.waiting.push(resolve);
      });
    }
    this.inFlight += 1;
    try {
      return await work();
    } finally {
      this.inFlight -= 1;
      const next = this.waiting.shift();
      if (next) next();
    }
  }

  private async _cached<T>(key: string, load: () => Promise<T>): Promise<T> {
    if (this.cacheTtlMs <= 0) return load();
    const hit = this.cache.get(key);
    if (hit && hit.expiresAt > Date.now()) return hit.value as T;
    const value = await load();
    this.cache.set(key, { expiresAt: Date.now() + this.cacheTtlMs, value });
    return value;
  }

  /**
   * One GET, returning the parsed body and the pagination headers.
   *
   * The headers come from the shared client's response observer, so they belong to
   * THIS response. That is what the per-URL header queue this replaced could not
   * promise: a response that arrived and then failed (a 500, a 409 empty repo)
   * used to stay queued under its URL, and the next request for that same URL
   * picked up the dead response's headers.
   */
  private async _get<T>(
    path: string,
    params?: Record<string, QueryValue>,
  ): Promise<{ body: T; headers: Headers | undefined }> {
    const target = `${FORGEJO_API_PREFIX}${path}`;
    return this._withSlot(async () => {
      let headers: Headers | undefined;
      const body = await this.client.get<T>(target, params, {
        onResponse: (res) => {
          headers = res.headers;
        },
      });
      return { body, headers };
    });
  }

  /**
   * A list endpoint: a BARE JSON array plus whatever pagination headers exist.
   *
   * The three exceptions to "bare array" are handled by their own methods
   * ({@link searchRepositories}, {@link getVersion}, {@link getNodeInfo}).
   */
  private async _getList<T>(
    path: string,
    params?: Record<string, QueryValue>,
  ): Promise<ForgejoPage<T>> {
    const { body, headers } = await this._get<unknown>(path, params);
    const link = parseLinkHeader(headers);
    const totalCount = parseTotalCount(headers);
    const page = Number(params?.["page"] ?? 1);
    const limit = Number(params?.["limit"] ?? this.maxLimit);
    return { items: requireArray<T>(body, `GET ${path}`), totalCount, link, limit, page };
  }

  private _ownerPath(username: string): string {
    return `/users/${encodeURIComponent(username)}`;
  }

  /* ---------------------------------------------------------------------- */
  /* Instance                                                              */
  /* ---------------------------------------------------------------------- */

  /** `GET /version`. The instance's Forgejo version. */
  async getVersion(): Promise<ForgejoVersion> {
    const { body } = await this._get<ForgejoVersion>("/version");
    return body;
  }

  /**
   * `GET /nodeinfo`, with the zero-usage caveat attached.
   *
   * Two things to know about this endpoint: the usage counters are all zero
   * unless `SHARE_USER_STATISTICS` is on, and the response is cached for 180s
   * server-side, so polling it faster measures nothing.
   */
  async getNodeInfo(): Promise<ForgejoNodeInfoResult> {
    try {
      const { body } = await this._get<ForgejoNodeInfo>("/nodeinfo");
      const trustworthy = isNodeinfoUsageTrustworthy(body);
      const caveats = [
        ...(trustworthy
          ? []
          : ["nodeinfo usage is all zeros; SHARE_USER_STATISTICS is disabled on this instance"]),
        "nodeinfo is cached server-side for 180s, so polling faster cannot observe a change",
        "ActiveHalfyear covers 6 months, and active means a recent login rather than recent activity",
      ];
      return { available: true, nodeinfo: body, usageTrustworthy: trustworthy, caveats };
    } catch (err) {
      if (err instanceof UpstreamError && err.status === 404) {
        return {
          available: false,
          usageTrustworthy: false,
          caveats: ["this instance does not expose nodeinfo (404)"],
        };
      }
      throw err;
    }
  }

  /* ---------------------------------------------------------------------- */
  /* Users                                                                 */
  /* ---------------------------------------------------------------------- */

  /** `GET /users/{username}`. Note `login`, not `username`, is the handle. */
  async getUser(username: string): Promise<ForgejoUser> {
    return this._cached(`user:${username}`, async () => {
      const { body } = await this._get<ForgejoUser>(this._ownerPath(username));
      return body;
    });
  }

  /** `GET /users`. */
  async listUsers(
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoUser>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._cached(`users:${options.page ?? 1}:${limit}`, () =>
      this._getList<ForgejoUser>("/users", { page: options.page ?? 1, limit }),
    );
  }

  /** `GET /users/search`. */
  async searchUsers(
    query: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoUser>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoUser>("/users/search", {
      q: query,
      page: options.page ?? 1,
      limit,
    });
  }

  /** `GET /users/{username}/repos`. */
  async listUserRepositories(
    username: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoRepository>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoRepository>(`${this._ownerPath(username)}/repos`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /**
   * `GET /users/{username}/heatmap`.
   *
   * An empty array here is ambiguous: it means EITHER "no activity" OR "no
   * access", and when `EnableUserHeatmap` is off the endpoint answers 404 rather
   * than 501. The result therefore carries a verdict instead of a bare array, so
   * an empty array is never rendered as a genuine zero by accident.
   *
   * The shape is fixed: 15-minute buckets over roughly 53 weeks, with no
   * parameters to change the window or the granularity.
   */
  async getUserHeatmap(username: string): Promise<ForgejoHeatmapResult> {
    try {
      const page = await this._getList<{ timestamp: number; contributions: number }>(
        `${this._ownerPath(username)}/heatmap`,
      );
      return { supported: true, entries: page.items, ambiguous: page.items.length === 0 };
    } catch (err) {
      if (err instanceof UpstreamError && err.status === 404) {
        // Feature detection by status: heatmaps are disabled on this instance.
        return { supported: false, entries: [], ambiguous: true };
      }
      throw err;
    }
  }

  /** `GET /users/{username}/activities/feeds`. */
  async listUserActivities(
    username: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoActivity>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoActivity>(`${this._ownerPath(username)}/activities/feeds`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /* ---------------------------------------------------------------------- */
  /* Repositories                                                          */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /repos/search` — the ONE list endpoint that wraps its results.
   *
   * It answers `{ok, data}`, so the bare-array reader would reject it. The `ok`
   * flag is surfaced rather than assumed true.
   */
  async searchRepositories(
    query: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoRepositorySearch> {
    const limit = clampLimit(options.limit, this.maxLimit);
    const { body } = await this._get<ForgejoRepositorySearch>("/repos/search", {
      q: query,
      page: options.page ?? 1,
      limit,
    });
    return {
      ok: body?.ok === true,
      data: requireArray<ForgejoRepository>(body?.data, "GET /repos/search"),
    };
  }

  /** `GET /repos/{owner}/{repo}`. */
  async getRepository(owner: string, repo: string): Promise<ForgejoRepository> {
    return this._cached(`repo:${owner}/${repo}`, async () => {
      const { body } = await this._get<ForgejoRepository>(repoPath(owner, repo));
      return body;
    });
  }

  /** `GET /repos/{owner}/{repo}/languages`. Byte counts keyed by language. */
  async getRepositoryLanguages(owner: string, repo: string): Promise<Record<string, number>> {
    return this._cached(`repo:${owner}/${repo}:languages`, async () => {
      const { body } = await this._get<Record<string, number>>(
        `${repoPath(owner, repo)}/languages`,
      );
      if (typeof body !== "object" || body === null) {
        throw new Error(`GET /repos/${owner}/${repo}/languages: expected an object`);
      }
      return body;
    });
  }

  /** `GET /repos/{owner}/{repo}/topics`. */
  async getRepositoryTopics(owner: string, repo: string): Promise<string[]> {
    return this._cached(`repo:${owner}/${repo}:topics`, async () => {
      const { body } = await this._get<string[]>(`${repoPath(owner, repo)}/topics`);
      return requireArray<string>(body, `GET /repos/${owner}/${repo}/topics`);
    });
  }

  /** `GET /repos/{owner}/{repo}/branches`. */
  async listBranches(
    owner: string,
    repo: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoBranch>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoBranch>(`${repoPath(owner, repo)}/branches`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /** `GET /repos/{owner}/{repo}/tags`. */
  async listTags(
    owner: string,
    repo: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoTag>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoTag>(`${repoPath(owner, repo)}/tags`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /** `GET /repos/{owner}/{repo}/stargazers`. */
  async listStargazers(
    owner: string,
    repo: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoUser>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoUser>(`${repoPath(owner, repo)}/stargazers`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /** `GET /repos/{owner}/{repo}/collaborators`. */
  async listCollaborators(
    owner: string,
    repo: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoUser>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoUser>(`${repoPath(owner, repo)}/collaborators`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /* ---------------------------------------------------------------------- */
  /* Issues and pull requests                                              */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /repos/{owner}/{repo}/issues`.
   *
   * `type=issues` is ALWAYS sent, and not as a default that can be turned off:
   * this endpoint returns PULL REQUESTS TOO otherwise, and the only way to tell
   * the two apart afterwards is `pull_request !== null` on every row. Filtering
   * client-side means downloading pull requests in full to throw them away.
   *
   * `state` defaults to `open` server-side, not `all`, so it is always explicit.
   *
   * `since` (RFC 3339, "only items updated after") is the incremental-sync
   * primitive; see {@link listIssuesUpdatedSince}.
   */
  async listIssues(
    owner: string,
    repo: string,
    options: {
      state?: "open" | "closed" | "all";
      since?: string;
      page?: number;
      limit?: number;
    } = {},
  ): Promise<ForgejoPage<ForgejoIssue>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoIssue>(`${repoPath(owner, repo)}/issues`, {
      state: options.state ?? "open",
      type: "issues",
      since: options.since,
      page: options.page ?? 1,
      limit,
    });
  }

  /**
   * Issues updated after an instant. The incremental-sync primitive.
   *
   * Neither upstream offers anything pagination-free except this: track the max
   * `updated_at` seen per repository (see {@link maxUpdatedAt}) and pass it back.
   */
  async listIssuesUpdatedSince(
    owner: string,
    repo: string,
    since: string,
    options: { state?: "open" | "closed" | "all"; page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoIssue>> {
    return this.listIssues(owner, repo, { ...options, since });
  }

  /** The newest `updated_at` in a page, or undefined for an empty page. */
  maxUpdatedAt(issues: readonly ForgejoIssue[]): string | undefined {
    let newest: string | undefined;
    for (const issue of issues) {
      if (typeof issue.updated_at !== "string") continue;
      if (newest === undefined || issue.updated_at > newest) newest = issue.updated_at;
    }
    return newest;
  }

  /** `GET /repos/issues/search`. `type=issues` is sent here too. */
  async searchIssues(
    query: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoIssue>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoIssue>("/repos/issues/search", {
      q: query,
      type: "issues",
      page: options.page ?? 1,
      limit,
    });
  }

  /** `GET /repos/{owner}/{repo}/pulls`. */
  async listPullRequests(
    owner: string,
    repo: string,
    options: { state?: "open" | "closed" | "all"; page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoPullRequest>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoPullRequest>(`${repoPath(owner, repo)}/pulls`, {
      state: options.state ?? "open",
      page: options.page ?? 1,
      limit,
    });
  }

  /* ---------------------------------------------------------------------- */
  /* Commits                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /repos/{owner}/{repo}/commits`.
   *
   * `stat`, `files` and `verification` all DEFAULT TO TRUE, so all three are
   * explicitly set to `false` on every request. Without that, one page of 50
   * commits arrives with 50 full diffs and 50 signature blobs attached.
   *
   * An EMPTY repository answers 409 `{"message":"Git Repository is empty."}`,
   * not `[]`, so that is caught and reported as `empty: true` rather than being
   * flattened into "no commits", which would be indistinguishable from a
   * repository whose history is inaccessible.
   *
   * `sha` and `not` are the other incremental primitive: `?sha=<ref>&not=<range>`
   * asks for what a ref adds on top of a range.
   */
  async listCommits(
    owner: string,
    repo: string,
    options: { sha?: string; not?: string; page?: number; limit?: number } = {},
  ): Promise<ForgejoCommitPage> {
    const limit = clampLimit(options.limit, this.maxLimit);
    try {
      const page = await this._getList<ForgejoCommit>(`${repoPath(owner, repo)}/commits`, {
        sha: options.sha,
        not: options.not,
        page: options.page ?? 1,
        limit,
        stat: false,
        files: false,
        verification: false,
      });
      return { commits: page.items, empty: false, totalCount: page.totalCount, link: page.link };
    } catch (err) {
      if (err instanceof UpstreamError && err.status === 409) return { commits: [], empty: true };
      throw err;
    }
  }

  /**
   * Contributors, DERIVED — there is no contributors endpoint.
   *
   * Forgejo's `GetContributorStats` is web-only and is not exposed by the API, so
   * this aggregates a commit listing instead. Two consequences, both inherent to
   * the source data rather than to this implementation:
   *
   * - A top-level `author` is a Forgejo user resolved BY EMAIL and is `null` for
   *   anyone who has never registered, so counting `author.login` alone
   *   undercounts. This merges the top-level resolution with the raw
   *   `commit.author` signature so unregistered committers still appear, marked
   *   `unregistered: true`.
   * - `author` and `committer` resolve independently and can be different users
   *   for one commit. Only `author` is counted, which matches the usual definition
   *   of a contributor.
   */
  async aggregateContributors(
    owner: string,
    repo: string,
    options: { maxPages?: number } = {},
  ): Promise<ForgejoContributor[]> {
    const maxPages = options.maxPages ?? 10;
    const byIdentity = new Map<string, ForgejoContributor>();

    for (let page = 1; page <= maxPages; page += 1) {
      const result = await this.listCommits(owner, repo, { page, limit: this.maxLimit });
      if (result.empty) break;
      for (const entry of result.commits) {
        const signature = entry.commit?.author;
        const login = entry.author?.login ?? null;
        const email = signature?.email ?? "";
        const name = entry.author?.full_name ?? signature?.name ?? email;
        // Identity is the login when there is one, else the email.
        const key = login ?? `email:${email}`;
        const existing = byIdentity.get(key);
        if (existing) existing.commits += 1;
        else {
          byIdentity.set(key, {
            login,
            name: typeof name === "string" ? name : email,
            email,
            commits: 1,
            unregistered: login === null,
          });
        }
      }
      if (result.commits.length < this.maxLimit) break;
    }

    return [...byIdentity.values()].sort((a, b) => b.commits - a.commits);
  }

  /* ---------------------------------------------------------------------- */
  /* Organizations                                                        */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /orgs`.
   *
   * The result is CACHED, and that is safe only because this cache belongs to one
   * service instance bound to one token. Org visibility WIDENS with privilege,
   * so one token's org list cached under a key another token reads would hand out
   * visibility the second token does not have. Never share this cache across
   * tokens.
   */
  async listOrganizations(
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoOrganization>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._cached(`orgs:${options.page ?? 1}:${limit}`, () =>
      this._getList<ForgejoOrganization>("/orgs", { page: options.page ?? 1, limit }),
    );
  }

  /** `GET /orgs/{org}`. The path segment is the org's `name`, its HANDLE. */
  async getOrganization(org: string): Promise<ForgejoOrganization> {
    return this._cached(`org:${org}`, async () => {
      const { body } = await this._get<ForgejoOrganization>(`/orgs/${encodeURIComponent(org)}`);
      return body;
    });
  }

  /** `GET /orgs/{org}/repos`. */
  async listOrganizationRepositories(
    org: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoRepository>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoRepository>(`/orgs/${encodeURIComponent(org)}/repos`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /** `GET /orgs/{org}/teams`. Read `units_map`, not the deprecated `units`. */
  async listOrganizationTeams(
    org: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoTeam>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoTeam>(`/orgs/${encodeURIComponent(org)}/teams`, {
      page: options.page ?? 1,
      limit,
    });
  }

  /** `GET /orgs/{org}/activities/feeds`. */
  async listOrganizationActivities(
    org: string,
    options: { page?: number; limit?: number } = {},
  ): Promise<ForgejoPage<ForgejoActivity>> {
    const limit = clampLimit(options.limit, this.maxLimit);
    return this._getList<ForgejoActivity>(`/orgs/${encodeURIComponent(org)}/activities/feeds`, {
      page: options.page ?? 1,
      limit,
    });
  }
}

declare module "cordis" {
  interface Context {
    forgejo: ForgejoAdapterService;
  }
}

/* -------------------------------------------------------------------------- */
/* Manifest                                                                    */
/* -------------------------------------------------------------------------- */

export const FORGEJO_SKILLS: PluginAgentSkill[] = [
  {
    id: "getInstanceInfo",
    name: "Get instance info",
    description:
      "Read the Forgejo version and nodeinfo. The nodeinfo usage counters are all zeros unless SHARE_USER_STATISTICS is enabled, so the result says whether they can be trusted.",
    tags: ["forgejo", "instance", "read"],
    examples: ["getInstanceInfo"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "searchRepositories",
    name: "Search repositories",
    description:
      "Search repositories. Note the counter field names: stars_count, forks_count, watchers_count, open_issues_count, open_pr_counter, release_counter.",
    tags: ["forgejo", "repositories", "search", "read"],
    examples: ['searchRepositories {"query":"platform"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getRepository",
    name: "Get repository",
    description: "Read one repository by owner and name.",
    tags: ["forgejo", "repositories", "read"],
    examples: ['getRepository {"owner":"acme","repo":"widgets"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listIssues",
    name: "List issues",
    description:
      "List a repository's issues, with type=issues always set so pull requests are excluded. Supports since=<RFC3339> for incremental sync.",
    tags: ["forgejo", "issues", "read"],
    examples: ['listIssues {"owner":"acme","repo":"widgets","state":"all"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listPullRequests",
    name: "List pull requests",
    description:
      "List a repository's pull requests. `merged` is a separate boolean from `state`, and the base/head branch names serialise as `label`.",
    tags: ["forgejo", "pulls", "read"],
    examples: ['listPullRequests {"owner":"acme","repo":"widgets","state":"all"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listCommits",
    name: "List commits",
    description:
      "List a repository's commits with diffs and signatures disabled. An empty repository answers 409 rather than an empty list, which the result reports as empty: true.",
    tags: ["forgejo", "commits", "read"],
    examples: ['listCommits {"owner":"acme","repo":"widgets"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listContributors",
    name: "List contributors",
    description:
      "Derive contributors by aggregating commits. There is no contributors endpoint — GetContributorStats is web-only — so unregistered committers are reconstructed from the raw git signature.",
    tags: ["forgejo", "commits", "metrics", "read"],
    examples: ['listContributors {"owner":"acme","repo":"widgets"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listOrganizations",
    name: "List organizations",
    description:
      "List organizations visible to this token. Visibility widens with privilege, so the same request returns different lists for different tokens.",
    tags: ["forgejo", "organizations", "read"],
    examples: ["listOrganizations"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getUserActivity",
    name: "Get user activity",
    description:
      "Read a user's activity feed, repositories, or 53-week contribution heatmap. The heatmap returns [] for both no activity and no access, so the result carries a verdict.",
    tags: ["forgejo", "users", "read"],
    examples: ['getUserActivity {"username":"ada"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const forgejoManifest: PluginManifest = {
  id: "forgejo",
  name: "Forgejo",
  description:
    "Read-only analytics over a Forgejo instance: repositories, issues, pull requests, commits and contributors.",
  version: "1.0.0",
  category: "upstream",
  uiPath: "/plugins/forgejo",
  icon: "git-branch",
  order: 31,
  defaultEnabled: true,
  upstream: { product: "Forgejo", envPrefix: "FORGEJO" },
  agent: {
    name: "Forgejo Agent",
    description:
      "Queries a Forgejo instance read-only with a personal access token. Forgejo has no API rate limiting, so the adapter limits its own concurrency and caches aggressively instead.",
    version: "1.0.0",
    skills: FORGEJO_SKILLS,
  },
};

function requireText(params: Record<string, unknown>, field: string, skill: string): string {
  const value = params[field];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${skill}: "${field}" is required and must be a non-empty string`);
  }
  return value;
}

function requireIssueState(value: unknown, skill: string): "open" | "closed" | "all" | undefined {
  return value === "open" || value === "closed" || value === "all" ? value : undefined;
}

/**
 * Skill handlers.
 *
 * `api.ctx` rather than `this`: a skill handler is a plain method on an object
 * literal, so `this` is the handler record and not the plugin's context.
 * `ctx.forgejo` can THROW rather than return undefined when the service is not
 * reachable, so the read is guarded and reports a usable message.
 */
function forgejoApi(ctx: Context): ForgejoAdapterService {
  try {
    return ctx.forgejo;
  } catch {
    throw new Error("forgejo adapter is not loaded");
  }
}

export const forgejoLoader: PluginLoader = {
  service: ForgejoAdapterService,
  skills: () => [
    {
      id: "getInstanceInfo",
      handle: async (_params, api) => {
        const forgejo = forgejoApi(api.ctx);
        const [version, nodeinfo] = await Promise.all([
          forgejo.getVersion(),
          forgejo.getNodeInfo(),
        ]);
        return { version, nodeinfo };
      },
    },
    {
      id: "searchRepositories",
      handle: async (params, api) =>
        forgejoApi(api.ctx).searchRepositories(requireText(params, "query", "searchRepositories"), {
          page: typeof params.page === "number" ? params.page : undefined,
          limit: typeof params.limit === "number" ? params.limit : undefined,
        }),
    },
    {
      id: "getRepository",
      handle: async (params, api) =>
        forgejoApi(api.ctx).getRepository(
          requireText(params, "owner", "getRepository"),
          requireText(params, "repo", "getRepository"),
        ),
    },
    {
      id: "listIssues",
      handle: async (params, api) =>
        forgejoApi(api.ctx).listIssues(
          requireText(params, "owner", "listIssues"),
          requireText(params, "repo", "listIssues"),
          {
            state: requireIssueState(params.state, "listIssues"),
            since: typeof params.since === "string" ? params.since : undefined,
            page: typeof params.page === "number" ? params.page : undefined,
            limit: typeof params.limit === "number" ? params.limit : undefined,
          },
        ),
    },
    {
      id: "listPullRequests",
      handle: async (params, api) =>
        forgejoApi(api.ctx).listPullRequests(
          requireText(params, "owner", "listPullRequests"),
          requireText(params, "repo", "listPullRequests"),
          {
            state: requireIssueState(params.state, "listPullRequests"),
            page: typeof params.page === "number" ? params.page : undefined,
            limit: typeof params.limit === "number" ? params.limit : undefined,
          },
        ),
    },
    {
      id: "listCommits",
      handle: async (params, api) =>
        forgejoApi(api.ctx).listCommits(
          requireText(params, "owner", "listCommits"),
          requireText(params, "repo", "listCommits"),
          {
            sha: typeof params.sha === "string" ? params.sha : undefined,
            not: typeof params.not === "string" ? params.not : undefined,
            page: typeof params.page === "number" ? params.page : undefined,
          },
        ),
    },
    {
      id: "listContributors",
      handle: async (params, api) =>
        forgejoApi(api.ctx).aggregateContributors(
          requireText(params, "owner", "listContributors"),
          requireText(params, "repo", "listContributors"),
        ),
    },
    {
      id: "listOrganizations",
      handle: async (_params, api) => forgejoApi(api.ctx).listOrganizations(),
    },
    {
      id: "getUserActivity",
      handle: async (params, api) => {
        const forgejo = forgejoApi(api.ctx);
        const username = requireText(params, "username", "getUserActivity");
        return {
          user: await forgejo.getUser(username),
          repositories: await forgejo.listUserRepositories(username),
          activities: await forgejo.listUserActivities(username),
          heatmap: await forgejo.getUserHeatmap(username),
        };
      },
    },
  ],
};
