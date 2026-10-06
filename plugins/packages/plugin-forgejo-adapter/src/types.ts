/**
 * Forgejo wire types.
 *
 * Verified against Forgejo 9.0.2. Everything below is a field that exists on the
 * wire in that version.
 *
 * THERE IS NO ENVELOPE
 * --------------------
 * Forgejo returns bare JSON. List endpoints return a BARE ARRAY, not
 * `{items: [...]}`. Three exceptions, and they are worth memorising because each
 * one is a different shape:
 * - `GET /repos/search` returns `{ok, data}`.
 * - Errors are `{message, url, errors?}`.
 * - `GET /version` returns a single object.
 *
 * A 500's `message` is blanked to `""` in production builds, so it is never a
 * reliable error string — the `url` field is.
 *
 * THE FIELD NAMES ARE THE TRAPS
 * -----------------------------
 * Forgejo's JSON does not follow its own REST conventions consistently, and a
 * wrong guess here does not error: it yields `undefined`. Every such field below
 * is annotated with the name it is NOT.
 */

/* -------------------------------------------------------------------------- */
/* Pagination                                                                  */
/* -------------------------------------------------------------------------- */

/**
 * The hard cap on `limit`, applied by SILENT CLAMPING.
 *
 * Asking for `limit=1000` returns 50 items and no error, no warning header and
 * no indication that anything was dropped. The default is 30. Commits are
 * clamped to the same 50.
 */
export const FORGEJO_MAX_LIMIT = 50;

/** Forgejo's default page size, used when a caller does not pass one. */
export const FORGEJO_DEFAULT_LIMIT = 30;

/**
 * `Link` (RFC 5988) pagination info.
 *
 * `lastPage` comes from `rel="last"` and is the only reliable "is there more"
 * signal, because `X-Total-Count` is ABSENT on some endpoints.
 */
export interface ForgejoLink {
  next?: string;
  prev?: string;
  first?: string;
  last?: string;
  lastPage?: number;
  nextPage?: number;
  prevPage?: number;
}

/* -------------------------------------------------------------------------- */
/* Users                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * A user.
 *
 * - `login` is the username. It is NOT `username` and NOT `name`.
 * - `active` is the account flag. It is NOT `is_active`.
 * - `created` has NO `_at` suffix.
 *
 * `is_admin`, `language`, `last_login`, `login_name`, `source_id` and
 * `prohibit_login` are populated ONLY for self or a site admin. A read-scoped
 * token will not see them, so this adapter does not build an "is admin" column
 * out of them. `email` is a placeholder unless the request is signed in.
 */
export interface ForgejoUser {
  id: number;
  /** THE username. */
  login: string;
  full_name?: string;
  email?: string;
  avatar_url?: string;
  /** NOT `is_active`. */
  active: boolean;
  is_admin?: boolean;
  /** No `_at` suffix. */
  created: string;
  restricted?: boolean;
  language?: string;
  last_login?: string;
  login_name?: string;
  source_id?: number;
  prohibit_login?: boolean;
  [key: string]: unknown;
}

/** A user as it appears NESTED under a repository's `owner`. */
export interface ForgejoRepositoryOwner {
  id: number;
  login: string;
  full_name?: string;
  email?: string;
  avatar_url?: string;
  active: boolean;
  created: string;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* Repositories                                                                */
/* -------------------------------------------------------------------------- */

/**
 * A repository.
 *
 * Counters are the traps here:
 * - `stars_count`, NOT `stars`
 * - `forks_count`, NOT `forks`
 * - `watchers_count`, NOT `watchers`
 * - `open_issues_count`, NOT `open_issues`
 * - `open_pr_counter` — `_counter`, NOT `_count`
 * - `release_counter` — SINGULAR `release`, not `releases`
 *
 * `name` is the bare repository name and `full_name` is `owner/repo`. `size` is
 * in KILOBYTES.
 */
export interface ForgejoRepository {
  id: number;
  name: string;
  full_name: string;
  owner: ForgejoRepositoryOwner;
  description?: string;
  private?: boolean;
  fork?: boolean;
  template?: boolean;
  empty?: boolean;
  archived?: boolean;
  mirror?: boolean;
  default_branch?: string;
  /** KB. */
  size: number;
  stars_count: number;
  forks_count: number;
  watchers_count: number;
  open_issues_count: number;
  /** `_counter`, not `_count`. */
  open_pr_counter: number;
  /** Singular `release`. */
  release_counter: number;
  created_at: string;
  updated_at: string;
  html_url?: string;
  ssh_url?: string;
  clone_url?: string;
  [key: string]: unknown;
}

/** `GET /repos/search` — the ONE list endpoint that wraps its results. */
export interface ForgejoRepositorySearch {
  ok: boolean;
  data: ForgejoRepository[];
}

/** `GET /repos/{o}/{r}/tags` entries. */
export interface ForgejoTag {
  name: string;
  id?: string;
  message?: string;
  commit?: { url?: string; sha?: string; created?: string };
  zipball_url?: string;
  tarball_url?: string;
  [key: string]: unknown;
}

/** `GET /repos/{o}/{r}/branches` entries. */
export interface ForgejoBranch {
  name: string;
  protected?: boolean;
  required_approvals?: number;
  enable_status_check?: boolean;
  user_can_push?: boolean;
  user_can_merge?: boolean;
  effective_branch_protection_name?: string;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* Issues and pull requests                                                    */
/* -------------------------------------------------------------------------- */

/**
 * An issue OR a pull request row.
 *
 * - `number`, NOT `index`
 * - `user` is the author, NOT `poster` or `author`
 * - `assets`, NOT `attachments`
 * - `due_date`, NOT `deadline`
 * - `comments` is a COUNT, not a list
 *
 * `pull_request` is the discriminator: `pull_request !== null` means the row is
 * actually a PULL REQUEST. `GET /repos/{o}/{r}/issues` returns pull requests too
 * unless `type=issues` is passed, which this adapter always does.
 */
export interface ForgejoIssue {
  id: number;
  /** NOT `index`. */
  number: number;
  title: string;
  /** Defaults to `open`, not `all`. */
  state: string;
  body?: string;
  /** NOT `poster` / `author`. */
  user: ForgejoUser;
  labels?: Array<{ id: number; name: string; color?: string; description?: string }>;
  assignees?: ForgejoUser[];
  assigner?: ForgejoUser | null;
  milestone?: { id: number; title: string; description?: string; state?: string } | null;
  comments: number;
  created_at: string;
  updated_at: string;
  closed_at?: string | null;
  /** NOT `deadline`. */
  due_date?: string | null;
  /** NOT `attachments`. */
  assets?: unknown[];
  /** `null` for a real issue; non-null when this row is actually a PR. */
  pull_request: unknown | null;
  repository?: { id: number; name: string; full_name?: string; owner?: ForgejoRepositoryOwner };
  url?: string;
  html_url?: string;
  [key: string]: unknown;
}

/**
 * A branch reference on a pull request.
 *
 * TRAP: `PRBranchInfo`'s branch name serialises as **`label`**, not `name`.
 */
export interface ForgejoPullRequestBranch {
  label: string;
  ref: string;
  sha: string;
  repo_id: number;
  repo?: { id: number; name: string; owner?: ForgejoRepositoryOwner; full_name?: string };
}

/**
 * A pull request.
 *
 * `merged` is a SEPARATE boolean from `state`: a merged PR has
 * `state: "closed"`, `merged: true` and a `merged_at`. Counting `state === "closed"`
 * as "merged" counts every abandoned PR too.
 */
export interface ForgejoPullRequest {
  id: number;
  number: number;
  title: string;
  state: string;
  /** Distinct from `state`. */
  merged: boolean;
  merged_at?: string | null;
  /** Branch name is `label`, not `name`. */
  base: ForgejoPullRequestBranch;
  /** Branch name is `label`, not `name`. */
  head: ForgejoPullRequestBranch;
  user: ForgejoUser;
  body?: string;
  created_at: string;
  updated_at: string;
  closed_at?: string | null;
  due_date?: string | null;
  changed_files?: number;
  additions?: number;
  deletions?: number;
  comments?: number;
  review_comments?: number;
  /** Nullable: open PRs have none. */
  merge_commit_sha?: string | null;
  html_url?: string;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* Commits                                                                     */
/* -------------------------------------------------------------------------- */

/** A raw git signature. `commit.author` / `commit.committer` are ALWAYS present. */
export interface ForgejoGitSignature {
  name: string;
  email: string;
  date: string;
}

/** One entry of `parents`. */
export interface ForgejoCommitParent {
  url: string;
  sha: string;
  created: string;
}

/**
 * A commit.
 *
 * THERE ARE THREE AUTHOR REPRESENTATIONS AT THREE NESTING LEVELS, and they are
 * not the same person:
 * - top-level `author` — a Forgejo user resolved BY EMAIL. `null` for an
 *   unregistered committer.
 * - top-level `committer` — resolved independently, so it can be a different
 *   user than `author` for the same commit.
 * - `commit.author` — the raw git signature. Always present.
 *
 * Consequence for analytics: contributor counts derived from `author.login`
 * UNDERCOUNT, because unregistered committers are `null` there while their
 * `commit.author.email` is still present. There is no contributors endpoint to
 * fall back on (`GetContributorStats` is web-only), so
 * {@link ForgejoAdapterService.aggregateContributors} merges all three.
 *
 * `stat`, `files` and `verification` all DEFAULT TO TRUE. Without an explicit
 * `false` for each, every commit in a listing carries its full diff and
 * signatures. This adapter always sends all three as false.
 */
export interface ForgejoCommit {
  sha: string;
  url?: string;
  /** Resolved user BY EMAIL, or null for an unregistered committer. */
  author: ForgejoUser | null;
  /** Resolved independently of `author`; may be a different user. */
  committer: ForgejoUser | null;
  commit: {
    author: ForgejoGitSignature;
    committer: ForgejoGitSignature;
    message: string;
    tree?: { url?: string; sha?: string };
    [key: string]: unknown;
  };
  parents: ForgejoCommitParent[];
  html_url?: string;
  /** Only present when `stat=true`. */
  stats?: { total: number; additions: number; deletions: number };
  /** Only present when `files=true`. */
  files?: unknown[];
  /** Only present when `verification=true`. */
  verification?: unknown;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* Organizations                                                               */
/* -------------------------------------------------------------------------- */

/**
 * An organization.
 *
 * `name` is the HANDLE (so `GET /orgs/{org}` takes the value of `name`, not of
 * the deprecated `username`). `created` has no `_at` suffix.
 *
 * Visibility of the `/orgs` LIST WIDENS with privilege, so the same request
 * returns different lists for different tokens. See
 * {@link ForgejoConfig}'s note on caching.
 */
export interface ForgejoOrganization {
  id: number;
  /** The handle. */
  name: string;
  description?: string;
  full_name?: string;
  avatar_url?: string;
  website?: string;
  location?: string;
  visibility?: string;
  /** DEPRECATED in favour of `name`. */
  username?: string;
  /** No `_at` suffix. */
  created: string;
  [key: string]: unknown;
}

/**
 * An organization team.
 *
 * `units` is DEPRECATED; `units_map` is the supported form and is a map from
 * unit name to the team's access level on that unit.
 */
export interface ForgejoTeam {
  id: number;
  name: string;
  organization?: ForgejoOrganization;
  description?: string;
  includes_all_repositories?: boolean;
  /** DEPRECATED. */
  units?: string[];
  units_map?: Record<string, number>;
  can_create_org_repo?: boolean;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* Activity feeds, nodeinfo, version                                           */
/* -------------------------------------------------------------------------- */

/** One entry of an activities feed. */
export interface ForgejoActivity {
  id: number;
  user_id: number;
  op_type: string;
  created_at: string;
  repo_id: number;
  repo_full_name?: string;
  repo_owner_name?: string;
  [key: string]: unknown;
}

/**
 * `GET /nodeinfo` usage block.
 *
 * TRAP: every counter here is ZERO unless the instance has
 * `[federation] SHARE_USER_STATISTICS` enabled. A zero is a successful response,
 * not an error and not a real measurement, so {@link isNodeinfoUsageTrustworthy}
 * exists to sanity-check it rather than have a dashboard draw a chart of it.
 *
 * Note that `ActiveHalfyear` is 6 months, not a year, and "active" means "has
 * logged in recently" — not "has done anything".
 */
export interface ForgejoNodeInfoUsage {
  users: {
    activeMonth: number;
    activeHalfyear: number;
    total: number;
  };
  localPosts: number;
  localComments: number;
}

/** `GET /nodeinfo`. */
export interface ForgejoNodeInfo {
  version: string;
  software: { name: string; version: string };
  protocols: string[];
  usage?: ForgejoNodeInfoUsage;
  openRegistrations?: boolean;
  metadata?: Record<string, unknown>;
}

/** `GET /version`. */
export interface ForgejoVersion {
  version: string;
}

/**
 * `GET /users/{username}/heatmap`.
 *
 * A bare ARRAY of 15-minute buckets covering roughly 53 weeks, with NO
 * parameters to change the window or the granularity.
 *
 * Two ways it lies, both handled by the adapter: an empty array means EITHER
 * "no access" OR "no activity", and when `EnableUserHeatmap` is off the endpoint
 * returns 404 (not 501), so feature detection is by status rather than by shape.
 */
export interface ForgejoHeatmapEntry {
  /** Unix seconds at the start of a 15-minute bucket. */
  timestamp: number;
  contributions: number;
}

/* -------------------------------------------------------------------------- */
/* Adapter results that carry a capability verdict                              */
/* -------------------------------------------------------------------------- */

/** `GET /users/{u}/heatmap`, with the ambiguity made explicit. */
export interface ForgejoHeatmapResult {
  /** False when the instance returned 404, i.e. heatmaps are disabled. */
  supported: boolean;
  /**
   * Always an empty array when `supported` is false. When it is true, an empty
   * array means "no activity", because "no access" was ruled out by the 404 check.
   */
  entries: ForgejoHeatmapEntry[];
  /**
   * True when an empty array cannot be interpreted: either heatmaps are off
   * server-side, or this token cannot see them. Do not render `[]` as a genuine
   * zero in that case.
   */
  ambiguous: boolean;
}

/** `GET /nodeinfo`, with the zero-usage caveat made explicit. */
export interface ForgejoNodeInfoResult {
  /** False when the instance has nodeinfo disabled (404). */
  available: boolean;
  nodeinfo?: ForgejoNodeInfo;
  /**
   * False when `usage` is absent or entirely zero, which is what a server with
   * `SHARE_USER_STATISTICS` off returns. Do not chart it when false.
   */
  usageTrustworthy: boolean;
  caveats: readonly string[];
}

/**
 * `GET /repos/{o}/{r}/commits`, with the empty-repository case made explicit.
 *
 * An empty repository answers 409 `{"message":"Git Repository is empty."}` — not
 * an empty array. Treating that as "no commits" and silently returning `[]` is
 * indistinguishable from a repository whose history is inaccessible.
 */
export interface ForgejoCommitPage {
  commits: ForgejoCommit[];
  /** True for the 409 empty-repository answer. */
  empty: boolean;
  /** From `X-Total-Count`; absent on some endpoints, hence optional. */
  totalCount?: number;
  link?: ForgejoLink;
}

/** One page of any other list endpoint. */
export interface ForgejoPage<T> {
  items: T[];
  totalCount?: number;
  link?: ForgejoLink;
  /** The `limit` actually sent, after clamping. */
  limit: number;
  page: number;
}

/** A contributor, derived by aggregating a commit listing. */
export interface ForgejoContributor {
  /** `login` when the email resolved to a Forgejo account, else null. */
  login: string | null;
  name: string;
  email: string;
  commits: number;
  /** True when no Forgejo account matched this git email. */
  unregistered: boolean;
}

/* -------------------------------------------------------------------------- */
/* Config                                                                      */
/* -------------------------------------------------------------------------- */

export interface ForgejoConfig {
  /** The instance root, e.g. `https://git.example.com`. `/api/v1` is appended. */
  baseUrl: string;
  /**
   * A personal access token, sent as `Authorization: token <pat>`.
   *
   * A PAT, not an app password: app passwords use HTTP Basic, and Basic plus 2FA
   * additionally requires an `X-Forgejo-OTP` header on EVERY request. The token is
   * never placed in the query string — `?token=` / `?access_token=` are
   * deprecated, removed in v13, and gated behind `DISABLE_QUERY_AUTH_TOKEN`.
   *
   * Recommended least-privilege scopes for this adapter, which only ever issues
   * GETs: `read:user`, `read:repository`, `read:issue`, `read:organization`,
   * `read:misc`, `public-only`.
   */
  token: string;
  /** Per-request timeout. Defaults to 30s via the shared client. */
  timeoutMs?: number;
  /**
   * Max simultaneous in-flight requests.
   *
   * Forgejo has NO API rate limiting — no 429, no `X-RateLimit-*` — so the only
   * limit that exists is the one an impatient dashboard imposes on itself.
   * Defaults to 4.
   */
  concurrency?: number;
  /**
   * TTL for the adapter's response cache, in milliseconds. Defaults to 60s.
   *
   * CACHING IS PER SERVICE INSTANCE, and a service instance is bound to exactly
   * one token. That is load-bearing for `GET /orgs`, whose visibility WIDENS with
   * privilege: caching one token's org list under a key another token reads would
   * leak the first token's visibility to the second. Never share a cache across
   * tokens.
   */
  cacheTtlMs?: number;
  /** Page size cap applied locally. Never above {@link FORGEJO_MAX_LIMIT}. */
  limit?: number;
}
