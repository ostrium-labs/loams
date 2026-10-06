/**
 * Zulip adapter.
 *
 * SCOPE: read-only. Nothing in this file writes to a realm.
 *
 * WHAT IS REUSED
 * --------------
 * All HTTP mechanics — auth header assembly, query encoding, timeouts,
 * error normalization — come from `@loams-plugins/plugin-upstream-http`. This file owns only
 * what is genuinely Zulip's: the result/msg envelope, message-id-anchor
 * pagination, the `narrow` encoding, and the rate-limit headers that Zulip
 * uses to tell a client it is about to be throttled.
 *
 * TWO THINGS THAT ARE WORTH READING BEFORE EDITING
 * -------------------------------------------------
 * 1. `/api/v1`, never `/json/`. The `/json/` prefix is the cookie-plus-CSRF
 *    path the web app itself uses; it is not the API path.
 * 2. The API-key username is the account's DELIVERY EMAIL. Zulip compares it
 *    case-insensitively against `user_profile.delivery_email`. `email` is a
 *    different address and a user id is not an address at all; either mistake is
 *    a 401 that looks exactly like a bad API key.
 */

import { Context, Service } from "cordis";
import {
  QueryValue,
  UpstreamClient,
  UpstreamError,
  UpstreamLogger,
  buildQuery,
  loggerFrom,
} from "@loams-plugins/plugin-upstream-http";
import type { PluginAgentSkill, PluginLoader, PluginManifest } from "@loams-plugins/core";
import {
  ZulipConfig,
  ZulipCustomField,
  ZulipDerivedMetrics,
  ZulipEmoji,
  ZulipMessage,
  ZulipMessagesPage,
  ZulipMessagesQuery,
  ZulipNarrowMatches,
  ZulipNarrowTerm,
  ZulipPresenceEntry,
  ZulipRateLimitState,
  ZulipRealmPresence,
  ZulipSelfProfile,
  ZulipStream,
  ZulipSuccess,
  ZulipTopic,
  ZulipUser,
  ZulipUserPresence,
} from "./types.js";

/**
 * The API prefix.
 *
 * `/json/` is the web app's cookie/CSRF path and is deliberately NOT used here.
 */
export const ZULIP_API_PREFIX = "/api/v1";

/** The server's hard ceiling on one `GET /messages` fetch. 1000 is the recommendation. */
export const MAX_MESSAGES_PER_FETCH = 5000;

/** Requests per window, shared across ALL endpoints. There is no read-only tier above it. */
export const ZULIP_RATE_LIMIT_PER_MINUTE = 200;

/** Default point at which the adapter pauses instead of racing into a 429. */
const DEFAULT_RATE_LIMIT_FLOOR = 10;

/** The limit window is 60s; never sleep past it waiting for a reset. */
const MAX_RATE_LIMIT_WAIT_MS = 65_000;

/** Default page size for the anchor loop. */
const DEFAULT_PAGE_SIZE = 100;

/** Hard stop for the anchor loop, so a bad narrow cannot page forever. */
const DEFAULT_MAX_PAGES = 50;

/* -------------------------------------------------------------------------- */
/* Errors and parsing helpers                                                  */
/* -------------------------------------------------------------------------- */

/** A `result: "error"` response, or a response whose shape was not understood. */
export class ZulipApiError extends Error {
  /** Zulip's machine-readable code, e.g. `RATE_LIMIT_HIT`. Absent on most errors. */
  readonly code?: string;

  constructor(message: string, code?: string) {
    super(message);
    this.name = "ZulipApiError";
    this.code = code;
  }
}

/**
 * `GET /users/me/{stream_id}/topics` and friends aside, `GET /users/{id}`
 * returns the user under a `user` key — the one place a Zulip user is wrapped.
 */
function requireObject<T>(value: unknown, what: string): T {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ZulipApiError(`${what}: expected an object from the server, got ${typeof value}`);
  }
  return value as T;
}

function requireArray<T>(value: unknown, what: string): T[] {
  if (!Array.isArray(value)) {
    throw new ZulipApiError(
      `${what}: expected an array from the server, got ${value === undefined ? "undefined" : typeof value}`,
    );
  }
  return value as T[];
}

function tryParseJsonObject(text: string): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(text) as unknown;
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return undefined;
    return parsed as Record<string, unknown>;
  } catch {
    return undefined;
  }
}

/**
 * The payload keys of an envelope, without the envelope keys themselves.
 *
 * The envelope is flat — data sits alongside `result`/`msg` — so an endpoint
 * whose payload IS an open map (`matches_narrow` returns one keyed by message id)
 * cannot simply have its envelope cast to the payload type. That cast would hand
 * back `result` and `msg` as if they were data entries.
 */
function envelopeExtras(result: ZulipSuccess<unknown>): Record<string, unknown> {
  const {
    result: _result,
    msg: _msg,
    code: _code,
    ignored_parameters_unsupported: _ignored,
    ...rest
  } = result as Record<string, unknown>;
  return rest;
}

/**
 * Decode a custom profile field's `field_data`.
 *
 * `field_data` is a JSON-ENCODED STRING. For a dropdown field it decodes to an
 * object with a `field_data` key holding the chosen value, which is why this
 * returns `unknown` rather than pretending to know the shape.
 */
export function parseProfileFieldData(field: ZulipCustomField): unknown {
  if (typeof field.field_data !== "string") return undefined;
  try {
    return JSON.parse(field.field_data) as unknown;
  } catch {
    return undefined;
  }
}

/**
 * Tell the two presence key spaces apart.
 *
 * `GET /users/{id}/presence` merges modern entries keyed by the STRINGIFIED user
 * id with legacy entries keyed by delivery email, in one flat object. The
 * discriminator is the presence of `active_timestamp`.
 */
export function isModernPresence(entry: ZulipPresenceEntry | undefined): boolean {
  return (
    typeof entry === "object" &&
    entry !== null &&
    typeof (entry as { active_timestamp?: unknown }).active_timestamp === "number"
  );
}

/**
 * Encode a narrow into a query parameter value, URL-encoded ONCE.
 *
 * `buildQuery` runs everything through `URLSearchParams`, which would encode a
 * pre-encoded value a second time (`%5B` → `%255B`) and the server would
 * receive the literal text of the encoding rather than the JSON. So the narrow
 * is encoded here and spliced into the path itself, and `buildQuery` only ever
 * sees the remaining parameters.
 */
export function encodeNarrow(narrow: readonly ZulipNarrowTerm[]): string {
  const terms = narrow.map((term) =>
    term.negated === undefined ? { operator: term.operator, operand: term.operand } : { ...term },
  );
  return encodeURIComponent(JSON.stringify(terms));
}

/** Build `path?narrow=<encoded>` with the remaining params appended. */
function withNarrow(
  path: string,
  narrow: readonly ZulipNarrowTerm[] | undefined,
  params: Record<string, QueryValue> | undefined,
): string {
  const rest = buildQuery(params);
  if (!narrow || narrow.length === 0) return rest ? `${path}${rest}` : path;
  const encoded = `narrow=${encodeNarrow(narrow)}`;
  const tail = rest.slice(1);
  return tail.length > 0 ? `${path}?${encoded}&${tail}` : `${path}?${encoded}`;
}

/* -------------------------------------------------------------------------- */
/* Service                                                                     */
/* -------------------------------------------------------------------------- */

export class ZulipAdapterService extends Service {
  static inject = [];

  readonly config: ZulipConfig;
  private readonly client: UpstreamClient;
  private rateLimitState: ZulipRateLimitState | undefined;
  /** Fingerprints of dropped-parameter reports already logged, to avoid per-poll spam. */
  private readonly reportedIgnoredParams = new Set<string>();
  /** Bound so it can be handed straight to `request` as the response observer. */
  private readonly recordRateLimit: (res: Response) => void;
  private readonly log: UpstreamLogger | undefined;

  constructor(ctx: Context, config: ZulipConfig) {
    super(ctx, "zulip");
    this.config = config;
    this.log = loggerFrom(ctx);

    // The username is the DELIVERY EMAIL, never `email` and never a user id.
    this.client = new UpstreamClient(
      {
        baseUrl: config.baseUrl.replace(/\/+$/, ""),
        auth: { kind: "basic", username: config.email, password: config.apiKey },
        timeoutMs: config.timeoutMs,
      },
      this.log,
    );

    this.recordRateLimit = (res) => this._recordRateLimit(res);
  }

  /** Last rate-limit state seen, or undefined before the first response. */
  get rateLimit(): ZulipRateLimitState | undefined {
    return this.rateLimitState;
  }

  /* ---------------------------------------------------------------------- */
  /* Plumbing                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * Read the rate-limit budget off a response, as the shared client's observer.
   *
   * Zulip's rate limiting is a header-driven protocol: `X-RateLimit-Remaining`
   * tells a well-behaved client how much budget is left, and waiting for a 429
   * instead means a dashboard that has already failed its user.
   *
   * WHY THIS DOES NOT MATCH ON THE URL, unlike the `fetch` wrapper it replaces:
   * the observer is handed the response of THIS request, so there is nothing to
   * match. The old module-scoped wrapper had no such guarantee — it saw a
   * response and a URL separately, and two concurrent requests to the same URL
   * could be paired up one slot out of order.
   *
   * All three headers are required: a partial set is a response that simply is
   * not reporting its budget, and inventing the missing half would throttle on a
   * number the server never sent.
   */
  private _recordRateLimit(res: Response): void {
    const read = (name: string): number | undefined => {
      const raw = res.headers.get(name);
      if (typeof raw !== "string") return undefined;
      const parsed = Number(raw);
      return Number.isFinite(parsed) ? parsed : undefined;
    };
    const limit = read("X-RateLimit-Limit");
    const remaining = read("X-RateLimit-Remaining");
    const reset = read("X-RateLimit-Reset");
    if (limit === undefined || remaining === undefined || reset === undefined) return;
    this.rateLimitState = { limit, remaining, reset, observedAt: Date.now() };
  }

  /**
   * Pause before a request if the previous response said the budget is spent.
   *
   * Self-throttling off `X-RateLimit-Remaining` is the intended behaviour: a 429
   * is a failure the dashboard's user sees, and the header is how the server says
   * it is coming. `reset` is a UNIX TIMESTAMP in seconds, so the wait is computed
   * against the clock rather than treated as a delta.
   */
  private async _awaitHeadroom(): Promise<void> {
    const state = this.rateLimitState;
    if (!state) return;
    const floor = this.config.rateLimitFloor ?? DEFAULT_RATE_LIMIT_FLOOR;
    if (state.remaining > floor) return;
    const waitMs = Math.min(Math.max(0, state.reset * 1000 - Date.now()), MAX_RATE_LIMIT_WAIT_MS);
    if (waitMs <= 0) return;
    this.log?.debug(
      `zulip: ${state.remaining}/${state.limit} requests left, waiting ${waitMs}ms for the reset`,
    );
    await new Promise<void>((resolve) => {
      setTimeout(resolve, waitMs);
    });
  }

  /**
   * Perform one request and unwrap the envelope.
   *
   * `ignored_parameters_unsupported` is logged on every response, not only on
   * error: Zulip reports it on success too, and it is the only signal that a
   * parameter this adapter sent was silently dropped.
   */
  private async _call<T>(
    path: string,
    options: { params?: Record<string, QueryValue>; narrow?: ZulipNarrowTerm[] } = {},
  ): Promise<ZulipSuccess<T>> {
    await this._awaitHeadroom();
    const target = withNarrow(`${ZULIP_API_PREFIX}${path}`, options.narrow, options.params);

    let raw: Record<string, unknown> | undefined;
    try {
      raw = (await this.client.get<unknown>(target, undefined, {
        onResponse: this.recordRateLimit,
      })) as Record<string, unknown> | undefined;
    } catch (err) {
      // Zulip answers a rejection with the SAME envelope it uses for success,
      // plus an HTTP status. The shared client turns any non-2xx into an
      // `UpstreamError` carrying the body, so re-read the envelope out of it
      // rather than losing the actionable `msg` — "Invalid API key" and
      // "API rate limit reached" are the two strings worth surfacing.
      if (err instanceof UpstreamError && err.status !== 0) {
        const envelope = tryParseJsonObject(err.body);
        if (envelope && envelope["result"] === "error") {
          throw new ZulipApiError(
            typeof envelope["msg"] === "string" && envelope["msg"].length > 0
              ? envelope["msg"]
              : err.message,
            typeof envelope["code"] === "string" ? envelope["code"] : undefined,
          );
        }
      }
      throw err;
    }

    if (typeof raw !== "object" || raw === null) {
      throw new ZulipApiError(`${path}: response was not a Zulip envelope`);
    }

    const ignored = raw["ignored_parameters_unsupported"];
    if (Array.isArray(ignored) && ignored.length > 0) {
      const names = ignored.filter((name): name is string => typeof name === "string");
      const key = `${path}:${names.join(",")}`;
      if (!this.reportedIgnoredParams.has(key) && this.reportedIgnoredParams.size < 64) {
        this.reportedIgnoredParams.add(key);
        this.log?.warn(
          `zulip: server ignored unsupported parameter(s) [${names.join(", ")}] on ${path}` +
            " — this adapter is sending a parameter this server version does not know",
        );
      }
    }

    const message = typeof raw["msg"] === "string" ? raw["msg"] : "";
    const code = typeof raw["code"] === "string" ? raw["code"] : undefined;
    if (raw["result"] === "error") throw new ZulipApiError(message || `${path} failed`, code);
    if (raw["result"] !== "success") {
      throw new ZulipApiError(`${path}: unexpected result ${JSON.stringify(raw["result"])}`);
    }
    return raw as ZulipSuccess<T>;
  }

  /* ---------------------------------------------------------------------- */
  /* Users                                                                 */
  /* ---------------------------------------------------------------------- */

  /**
   * Every user in the realm.
   *
   * TRAP: the collection is keyed `members`, NOT `users`. `GET /users/{id}`
   * wraps its single result in `user`, and `GET /users/me` has neither key — it
   * spreads the fields flat. Three different shapes for one entity.
   */
  async listUsers(options: { includeCustomProfileFields?: boolean } = {}): Promise<ZulipUser[]> {
    const result = await this._call<{ members?: ZulipUser[] }>("/users", {
      params: { include_custom_profile_fields: options.includeCustomProfileFields },
    });
    return requireArray<ZulipUser>(result.members, "GET /users: the collection is keyed `members`");
  }

  /** One user by id or email. The result is wrapped in a `user` key here. */
  async getUser(idOrEmail: number | string): Promise<ZulipUser> {
    const result = await this._call<{ user?: ZulipUser }>(
      `/users/${encodeURIComponent(idOrEmail)}`,
    );
    return requireObject<ZulipUser>(
      result.user,
      "GET /users/{id}: the user is wrapped in a `user` key",
    );
  }

  /**
   * The authenticated user.
   *
   * TRAP: user fields are FLAT at the top level, and the one addition over
   * `ZulipUser` is `max_message_id` — an upper bound on the message count, not
   * a count, because message ids have gaps.
   */
  async getSelf(): Promise<ZulipSelfProfile> {
    const result = await this._call<ZulipSelfProfile>("/users/me");
    return result as ZulipSelfProfile;
  }

  /* ---------------------------------------------------------------------- */
  /* Streams (channels)                                                     */
  /* ---------------------------------------------------------------------- */

  /**
   * Every stream the user can see, sorted by name.
   *
   * `includeDefault` maps to the `include_default` parameter, whose only effect
   * is to ADD a per-stream `is_default` boolean. There is no
   * `include_default_subscriptions` parameter on this endpoint — sending that
   * name yields an `ignored_parameters_unsupported` entry and nothing else.
   */
  async listStreams(options: { includeDefault?: boolean } = {}): Promise<ZulipStream[]> {
    const result = await this._call<{ streams?: ZulipStream[] }>("/streams", {
      params: { include_default: options.includeDefault },
    });
    return requireArray<ZulipStream>(result.streams, "GET /streams");
  }

  /**
   * Resolve a channel NAME to its id.
   *
   * TRAP: the parameter is `stream`, not `stream_name`.
   */
  async getStreamId(stream: string): Promise<number> {
    const result = await this._call<{ stream_id?: number }>("/get_stream_id", {
      params: { stream },
    });
    if (typeof result.stream_id !== "number") {
      throw new ZulipApiError(`GET /get_stream_id: no stream_id for "${stream}"`);
    }
    return result.stream_id;
  }

  /**
   * Who is subscribed to a stream.
   *
   * TRAP: `subscribers` holds USER IDS, not usernames. Join against
   * `listUsers()` to render names.
   */
  async getStreamSubscribers(streamId: number): Promise<number[]> {
    const result = await this._call<{ subscribers?: number[] }>(`/streams/${streamId}/members`);
    return requireArray<number>(result.subscribers, "GET /streams/{id}/members");
  }

  /**
   * Every topic in a stream, sorted by `max_id` DESCENDING.
   *
   * ONE REQUEST PER STREAM, with no pagination. On a realm with hundreds of
   * channels this is the adapter's largest source of rate-limit pressure, which
   * is why `listAllTopics` below walks it under the shared throttler rather than
   * in a caller loop.
   */
  async listTopics(streamId: number): Promise<ZulipTopic[]> {
    const result = await this._call<{ topics?: ZulipTopic[] }>(`/users/me/${streamId}/topics`);
    return requireArray<ZulipTopic>(result.topics, "GET /users/me/{stream_id}/topics");
  }

  /** Topics for several streams, sequentially, so the rate-limit budget is not blown at once. */
  async listAllTopics(streamIds: readonly number[]): Promise<Map<number, ZulipTopic[]>> {
    const byStream = new Map<number, ZulipTopic[]>();
    for (const streamId of streamIds) {
      byStream.set(streamId, await this.listTopics(streamId));
    }
    return byStream;
  }

  /* ---------------------------------------------------------------------- */
  /* Messages                                                               */
  /* ---------------------------------------------------------------------- */

  /** One anchored page of messages. See {@link fetchAllMessages} for the loop. */
  async fetchMessages(query: ZulipMessagesQuery): Promise<ZulipMessagesPage> {
    if (query.message_ids !== undefined) {
      // `message_ids` is mutually exclusive with the anchor parameters, and the
      // variant returns no `anchor` at all.
      const result = await this._call<ZulipMessagesPage>("/messages", {
        params: { message_ids: query.message_ids },
        narrow: query.narrow,
      });
      return {
        messages: requireArray(result.messages, "GET /messages (message_ids)"),
        found_anchor: result.found_anchor === true,
        found_oldest: result.found_oldest === true,
        found_newest: result.found_newest === true,
        history_limited: result.history_limited === true,
      };
    }

    const result = await this._call<ZulipMessagesPage>("/messages", {
      params: {
        anchor: query.anchor,
        anchor_date: query.anchor_date,
        num_before: query.num_before,
        num_after: query.num_after,
      },
      narrow: query.narrow,
    });
    return {
      messages: requireArray(result.messages, "GET /messages"),
      found_anchor: result.found_anchor === true,
      found_oldest: result.found_oldest === true,
      found_newest: result.found_newest === true,
      history_limited: result.history_limited === true,
      anchor: typeof result.anchor === "number" ? result.anchor : undefined,
    };
  }

  /**
   * Walk backwards from an anchor until the start of history.
   *
   * Zulip's pagination is message-ID ANCHORING, not offsets and not cursors:
   * there are no totals, no page numbers and no `has_more`. The loop feeds each
   * page's returned `anchor` back in with `num_after: 0` and stops when
   * `found_oldest` is true.
   *
   * Two exits other than `found_oldest`, both defensive: a page that repeats its
   * own anchor has stopped making progress and looping on it would never
   * terminate, and `maxPages` stops a narrow that simply matches a very large
   * number of messages.
   */
  async fetchAllMessages(
    query: ZulipMessagesQuery & { pageSize?: number } = {},
  ): Promise<ZulipMessage[]> {
    const { pageSize = DEFAULT_PAGE_SIZE, ...rest } = query;
    const before = Math.min(pageSize, MAX_MESSAGES_PER_FETCH);
    const maxPages = this.config.maxPages ?? DEFAULT_MAX_PAGES;
    const messages: ZulipMessage[] = [];
    let anchor = rest.anchor ?? "newest";

    for (let page = 0; page < maxPages; page += 1) {
      const result = await this.fetchMessages({
        ...rest,
        anchor,
        num_before: before,
        num_after: 0,
      });
      messages.push(...result.messages);
      if (result.found_oldest) return messages;

      const next = result.anchor;
      if (next === undefined) {
        this.log?.warn(
          "zulip: GET /messages returned no anchor before found_oldest; stopping the anchor loop",
        );
        return messages;
      }
      if (next === anchor) {
        // The page came back anchored exactly where we asked from, so the next
        // request would be identical. Looping on it would never terminate.
        this.log?.warn(`zulip: anchor ${String(next)} did not advance; stopping the anchor loop`);
        return messages;
      }
      anchor = next;
    }
    throw new Error(
      `zulip: anchor loop stopped after ${maxPages} pages without reaching found_oldest; ` +
        "narrow the query or raise maxPages",
    );
  }

  /**
   * Full-text search.
   *
   * There is NO `/messages/search` endpoint. Search is `GET /messages` with a
   * single `search` narrow, and this method is the only place that knows it.
   */
  async searchMessages(query: string, options: { limit?: number } = {}): Promise<ZulipMessage[]> {
    const page = await this.fetchMessages({
      narrow: [{ operator: "search", operand: query }],
      anchor: "newest",
      num_before: Math.min(options.limit ?? DEFAULT_PAGE_SIZE, MAX_MESSAGES_PER_FETCH),
      num_after: 0,
    });
    return page.messages;
  }

  /** Fetch specific messages by id. No anchor is involved or returned. */
  async getMessagesByIds(ids: readonly number[]): Promise<ZulipMessage[]> {
    if (ids.length === 0) return [];
    const page = await this.fetchMessages({ message_ids: [...ids] });
    return page.messages;
  }

  /**
   * Which of the messages a narrow describes actually exist.
   *
   * TRAP: this is a membership test, not a search — the result is keyed by
   * message id as a STRING and each value is `{match_content, match_subject}`.
   * The second key is `match_subject`, NOT `match_topic`.
   */
  async matchNarrow(narrow: readonly ZulipNarrowTerm[]): Promise<ZulipNarrowMatches> {
    const result = await this._call<Record<string, unknown>>("/messages/matches_narrow", {
      narrow: [...narrow],
    });
    return envelopeExtras(result) as ZulipNarrowMatches;
  }

  /* ---------------------------------------------------------------------- */
  /* Realm metadata                                                         */
  /* ---------------------------------------------------------------------- */

  /**
   * Custom profile fields.
   *
   * `field_data` is a JSON-ENCODED STRING per field; use
   * {@link parseProfileFieldData} rather than assuming an object.
   */
  async listCustomProfileFields(): Promise<ZulipCustomField[]> {
    const result = await this._call<{ custom_fields?: ZulipCustomField[] }>(
      "/realm/profile_fields",
    );
    return requireArray<ZulipCustomField>(result.custom_fields, "GET /realm/profile_fields");
  }

  /**
   * Realm custom emoji.
   *
   * TRAP: `emoji` is an OBJECT keyed by emoji id as a STRING, and each value's
   * own `id` is a string too.
   */
  async listCustomEmoji(): Promise<Record<string, ZulipEmoji>> {
    const result = await this._call<{ emoji?: Record<string, ZulipEmoji> }>("/realm/emoji");
    return requireObject<Record<string, ZulipEmoji>>(result.emoji, "GET /realm/emoji");
  }

  /* ---------------------------------------------------------------------- */
  /* Presence                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * One user's presence.
   *
   * TRAP: `presence` merges two key spaces — modern entries keyed by STRINGIFIED
   * user id, legacy entries keyed by delivery email. And bots are REJECTED here
   * with a 400, not a 401, so a bot account is a client error here rather than
   * an auth problem.
   */
  async getUserPresence(idOrEmail: number | string): Promise<ZulipUserPresence> {
    const result = await this._call<ZulipUserPresence>(
      `/users/${encodeURIComponent(idOrEmail)}/presence`,
    );
    return {
      presence: requireObject<Record<string, ZulipPresenceEntry>>(
        result.presence,
        "GET /users/{id}/presence",
      ),
      server_timestamp: result.server_timestamp,
    };
  }

  /** Every presence in the realm, same two key spaces as {@link getUserPresence}. */
  async getRealmPresence(): Promise<ZulipRealmPresence> {
    const result = await this._call<ZulipRealmPresence>("/realm/presence");
    return {
      presences: requireObject<Record<string, ZulipPresenceEntry>>(
        result.presences,
        "GET /realm/presence",
      ),
      server_timestamp: result.server_timestamp,
    };
  }

  /* ---------------------------------------------------------------------- */
  /* Derived metrics                                                        */
  /* ---------------------------------------------------------------------- */

  /**
   * Metrics derived from ordinary reads.
   *
   * Zulip has NO aggregate or statistics endpoint — no DAU, no MAU, no
   * `realm_daily_active_users`; those names do not exist in the server. This
   * method aggregates two ordinary reads and returns the caveats alongside the
   * numbers, so a caller cannot render an upper bound as a count.
   */
  async deriveMetrics(): Promise<ZulipDerivedMetrics> {
    const [self, streams, members] = await Promise.all([
      this.getSelf(),
      this.listStreams(),
      this.listUsers(),
    ]);

    let activeHumanUsers = 0;
    let botCount = 0;
    let guestCount = 0;
    let adminCount = 0;
    let deactivatedUserCount = 0;
    for (const member of members) {
      if (member.is_active === false) {
        deactivatedUserCount += 1;
        continue;
      }
      if (member.is_bot) botCount += 1;
      else if (member.is_guest) guestCount += 1;
      else {
        activeHumanUsers += 1;
        if (member.is_admin === true) adminCount += 1;
      }
    }

    return {
      messageIdUpperBound: self.max_message_id,
      streamCount: streams.length,
      activeHumanUsers,
      botCount,
      guestCount,
      adminCount,
      deactivatedUserCount,
      caveats: [
        "Zulip exposes no aggregate or statistics endpoint: DAU and MAU are not available at all.",
        "messageIdUpperBound is GET /users/me max_message_id, an upper bound only — message ids have gaps.",
        "User counts are aggregated from GET /users members[], not read from a statistics API.",
      ],
    };
  }
}

declare module "cordis" {
  interface Context {
    zulip: ZulipAdapterService;
  }
}

/* -------------------------------------------------------------------------- */
/* Manifest                                                                    */
/* -------------------------------------------------------------------------- */

export const ZULIP_SKILLS: PluginAgentSkill[] = [
  {
    id: "listChannels",
    name: "List channels",
    description: "List every Zulip stream with its stream_id, name, description and privacy.",
    tags: ["zulip", "channels", "read"],
    examples: ["listChannels", 'listChannels {"includeDefault":true}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listTopics",
    name: "List topics",
    description:
      "List topics in one channel with each topic's latest message id. One request per channel; Zulip does not paginate this endpoint.",
    tags: ["zulip", "topics", "read"],
    examples: ['listTopics {"streamId":5}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "searchMessages",
    name: "Search messages",
    description:
      "Full-text search across messages. Zulip has no /messages/search endpoint; this narrows GET /messages with a search operator.",
    tags: ["zulip", "search", "read"],
    examples: ['searchMessages {"query":"onboarding"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getMessages",
    name: "Get messages",
    description:
      "Read messages by message-id anchor, walking backwards from an anchor (a message id, 'newest', 'oldest', 'first_unread' or a date) until the start of history.",
    tags: ["zulip", "messages", "read"],
    examples: ['getMessages {"streamId":5,"topic":"general"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listUsers",
    name: "List users",
    description: "List realm members with active, bot, guest and admin flags.",
    tags: ["zulip", "users", "read"],
    examples: ["listUsers"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getRealmMetrics",
    name: "Get realm metrics",
    description:
      "Derived realm metrics: message-id upper bound, channel count, and user counts by type. Zulip has no statistics endpoint, so no DAU or MAU is available.",
    tags: ["zulip", "metrics", "read"],
    examples: ["getRealmMetrics"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getPresence",
    name: "Get presence",
    description:
      "Read presence for one user or the whole realm. Bots are rejected by this endpoint, and modern entries are keyed by stringified user id while legacy entries are keyed by delivery email.",
    tags: ["zulip", "presence", "read"],
    examples: ['getPresence {"userId":3001}', "getPresence"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const zulipManifest: PluginManifest = {
  id: "zulip",
  name: "Zulip",
  description:
    "Read-only analytics over a Zulip realm: channels, topics, message search and user metrics.",
  version: "1.0.0",
  category: "upstream",
  uiPath: "/plugins/zulip",
  icon: "chat",
  order: 30,
  defaultEnabled: true,
  upstream: { product: "Zulip", envPrefix: "ZULIP" },
  agent: {
    name: "Zulip Agent",
    description:
      "Queries a Zulip realm read-only. Authenticates with an API key as the account's delivery email, and self-throttles against the shared 200 requests/60s limit.",
    version: "1.0.0",
    skills: ZULIP_SKILLS,
  },
};

function requireNumber(params: Record<string, unknown>, field: string, skill: string): number {
  const value = params[field];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${skill}: "${field}" is required and must be a number`);
  }
  return value;
}

function requireText(params: Record<string, unknown>, field: string, skill: string): string {
  const value = params[field];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${skill}: "${field}" is required and must be a non-empty string`);
  }
  return value;
}

/**
 * Skill handlers.
 *
 * `api.ctx` rather than `this`: a skill handler is a plain method on an object
 * literal, so `this` is the handler record and not the plugin's context.
 * `ctx.zulip` can THROW rather than return undefined if the service is not
 * reachable (cordis resolves an inherited context key only where it was
 * declared in `inject`), so the read is guarded and reports a usable message.
 */
function zulipApi(ctx: Context): ZulipAdapterService {
  try {
    return ctx.zulip;
  } catch {
    throw new Error("zulip adapter is not loaded");
  }
}

export const zulipLoader: PluginLoader = {
  service: ZulipAdapterService,
  skills: () => [
    {
      id: "listChannels",
      handle: async (params, api) =>
        zulipApi(api.ctx).listStreams({ includeDefault: params.includeDefault === true }),
    },
    {
      id: "listTopics",
      handle: async (params, api) =>
        zulipApi(api.ctx).listTopics(requireNumber(params, "streamId", "listTopics")),
    },
    {
      id: "searchMessages",
      handle: async (params, api) =>
        zulipApi(api.ctx).searchMessages(requireText(params, "query", "searchMessages"), {
          limit: typeof params.limit === "number" ? params.limit : undefined,
        }),
    },
    {
      id: "getMessages",
      handle: async (params, api) => {
        const narrow: ZulipNarrowTerm[] = [];
        if (typeof params.streamId === "number") {
          narrow.push({ operator: "channel", operand: params.streamId });
        }
        if (typeof params.topic === "string" && params.topic.length > 0) {
          narrow.push({ operator: "topic", operand: params.topic });
        }
        return zulipApi(api.ctx).fetchAllMessages({
          narrow: narrow.length > 0 ? narrow : undefined,
          anchor: typeof params.anchorDate === "string" ? "date" : "newest",
          anchor_date: typeof params.anchorDate === "string" ? params.anchorDate : undefined,
          pageSize: typeof params.pageSize === "number" ? params.pageSize : undefined,
        });
      },
    },
    { id: "listUsers", handle: async (_params, api) => zulipApi(api.ctx).listUsers() },
    { id: "getRealmMetrics", handle: async (_params, api) => zulipApi(api.ctx).deriveMetrics() },
    {
      id: "getPresence",
      handle: async (params, api) => {
        const user = params.userId ?? params.email;
        if (typeof user === "string" || typeof user === "number") {
          return zulipApi(api.ctx).getUserPresence(user);
        }
        return zulipApi(api.ctx).getRealmPresence();
      },
    },
  ],
};

export { UpstreamError };
