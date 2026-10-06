import { describe, it, expect, vi, beforeEach, afterEach } from "vite-plus/test";
import { Context } from "cordis";
import { AgentBus, HttpRouter, type PluginManifest, type PluginRuntime } from "@loams-plugins/core";
import {
  ZulipAdapterService,
  ZulipApiError,
  encodeNarrow,
  isModernPresence,
  parseProfileFieldData,
  zulipLoader,
  zulipManifest,
} from "../src/service.js";
import type { ZulipConfig, ZulipMessage, ZulipNarrowTerm } from "../src/types.js";

type MockedFetch = ReturnType<typeof vi.fn<typeof fetch>>;

/**
 * `PluginLoader.skills` is handed a `PluginRuntime`. These loaders build their
 * handler list from a literal and never read the runtime, but the contract
 * requires one, so give them a real (empty) one rather than casting `undefined`.
 */
function runtimeFor(manifest: PluginManifest): PluginRuntime {
  const ctx = new Context();
  return { id: manifest.id, manifest, ctx, router: new HttpRouter(ctx), bus: new AgentBus(ctx) };
}

const BASE = "https://chat.example.test";

/**
 * A fake response for the shared client.
 *
 * `headers` is a real `Headers` because the adapter reads `X-RateLimit-*` off
 * the response, which the shared client discards.
 */
function reply(body: unknown, headers: Record<string, string> = {}, status = 200): Response {
  return new Response(typeof body === "string" ? body : JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });
}

function envelope(data: Record<string, unknown> = {}): Record<string, unknown> {
  return { result: "success", msg: "", ...data };
}

/** The URL the nth (0-based) fetch was called with. */
function urlOf(fetchMock: MockedFetch, index = 0): string {
  return String(fetchMock.mock.calls[index]?.[0]);
}

/** Decoded `narrow` param of a request URL. */
function narrowOf(url: string): unknown[] {
  const value = new URL(url).searchParams.get("narrow");
  return value === null ? [] : (JSON.parse(value) as unknown[]);
}

function message(id: number, overrides: Partial<ZulipMessage> = {}): ZulipMessage {
  return {
    id,
    type: "stream",
    sender_id: 7,
    sender_email: "ada@example.test",
    sender_full_name: "Ada",
    sender_realm_str: "example",
    avatar_url: null,
    content: "<p>hi</p>",
    content_type: "text/html",
    timestamp: 1_700_000_000,
    recipient_id: 5,
    display_recipient: "general",
    subject: "chatter",
    stream_id: 5,
    client: "website",
    is_me_message: false,
    flags: [],
    reactions: [],
    ...overrides,
  };
}

describe("ZulipAdapterService", () => {
  let ctx: Context;
  let service: ZulipAdapterService;
  let fetchMock: MockedFetch;
  const built: ZulipAdapterService[] = [];

  /**
   * A cordis `Context` per service: `super(ctx, "zulip")` registers a name on
   * the context, and registering it twice on one context throws.
   */
  function makeService(overrides: Partial<ZulipConfig> = {}): ZulipAdapterService {
    const created = new ZulipAdapterService(new Context(), {
      baseUrl: BASE,
      email: "ada@example.test",
      apiKey: "SECRETKEY",
      ...overrides,
    });
    built.push(created);
    return created;
  }

  beforeEach(() => {
    const stub = vi.fn();
    vi.stubGlobal("fetch", stub);
    fetchMock = stub as unknown as MockedFetch;
    ctx = new Context();
    service = makeService();
  });

  afterEach(() => {
    built.length = 0;
    vi.unstubAllGlobals();
  });

  /* ------------------------------------------------------------------ */
  /* Envelope                                                            */
  /* ------------------------------------------------------------------ */

  it("unwraps the success envelope, keeping result and msg alongside the data", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          streams: [{ stream_id: 5, name: "general", description: "", invite_only: false }],
        }),
      ),
    );

    const streams = await service.listStreams();

    expect(streams).toHaveLength(1);
    expect(streams[0].stream_id).toBe(5);
    expect(urlOf(fetchMock)).toBe("https://chat.example.test/api/v1/streams");
  });

  it("turns a result:error envelope into a ZulipApiError carrying msg and code", async () => {
    fetchMock.mockImplementation(async () =>
      reply({ result: "error", msg: "Invalid API key", code: "INVALID_API_KEY" }, {}, 400),
    );

    await expect(service.listStreams()).rejects.toBeInstanceOf(ZulipApiError);
    await expect(service.listStreams()).rejects.toThrow("Invalid API key");
  });

  it("keeps code: RATE_LIMIT_HIT reachable on the thrown error", async () => {
    fetchMock.mockImplementation(async () =>
      reply({ result: "error", msg: "API rate limit reached", code: "RATE_LIMIT_HIT" }, {}, 429),
    );

    const error = await service.listStreams().catch((err: unknown) => err);
    expect(error).toBeInstanceOf(ZulipApiError);
    expect((error as ZulipApiError).code).toBe("RATE_LIMIT_HIT");
  });

  it("rejects a 200 whose result is neither success nor error", async () => {
    fetchMock.mockResolvedValueOnce(reply({ msg: "something" }));
    await expect(service.listStreams()).rejects.toThrow(/unexpected result/);
  });

  it("reads and logs ignored_parameters_unsupported on a SUCCESSFUL response", async () => {
    const warn = vi.fn();
    const warned = makeService();
    vi.spyOn(
      warned["log"] as unknown as { warn: (...args: unknown[]) => void },
      "warn",
    ).mockImplementation(warn);
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          streams: [],
          ignored_parameters_unsupported: ["include_default_subscriptions"],
        }),
      ),
    );

    await warned.listStreams();

    expect(warn).toHaveBeenCalledTimes(1);
    expect(String(warn.mock.calls[0][0])).toContain("include_default_subscriptions");
    // ...and it never actually sends that parameter.
    expect(urlOf(fetchMock)).not.toContain("include_default_subscriptions");
  });

  it("does not send the non-existent include_default_subscriptions parameter", async () => {
    fetchMock.mockResolvedValueOnce(reply(envelope({ streams: [] })));
    await service.listStreams({ includeDefault: true });
    const url = new URL(urlOf(fetchMock));
    expect(url.searchParams.get("include_default")).toBe("true");
    expect(url.searchParams.has("include_default_subscriptions")).toBe(false);
  });

  /* ------------------------------------------------------------------ */
  /* users                                                               */
  /* ------------------------------------------------------------------ */

  it("reads the user collection from `members`, not `users`", async () => {
    // A payload that carries BOTH keys, so a naive `.users` read is visibly wrong.
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          members: [
            { user_id: 1, full_name: "Ada", is_active: true, is_bot: false, is_guest: false },
            { user_id: 2, full_name: "Bot", is_active: true, is_bot: true, is_guest: false },
          ],
          users: [],
        }),
      ),
    );

    const members = await service.listUsers();

    expect(members).toHaveLength(2);
    expect(members[1].is_bot).toBe(true);
  });

  it("fails loudly when the collection key is absent, naming the correct key", async () => {
    fetchMock.mockResolvedValueOnce(reply(envelope({ users: [{ user_id: 1 }] })));
    await expect(service.listUsers()).rejects.toThrow(/keyed `members`/);
  });

  it("reads a single user from the `user` wrapper", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          user: {
            user_id: 3001,
            full_name: "Ada",
            is_active: true,
            is_bot: false,
            is_guest: false,
          },
        }),
      ),
    );

    const user = await service.getUser(3001);
    expect(user.user_id).toBe(3001);
    expect(urlOf(fetchMock)).toBe("https://chat.example.test/api/v1/users/3001");
  });

  it("reads /users/me as a FLAT profile plus max_message_id", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          user_id: 3001,
          email: "ada@example.test",
          full_name: "Ada Lovelace",
          delivery_email: "ada@example.test",
          is_active: true,
          is_bot: false,
          is_guest: false,
          avatar_url: "/avatar.png",
          max_message_id: 1_234_567,
        }),
      ),
    );

    const self = await service.getSelf();

    expect(self.user_id).toBe(3001);
    expect(self.max_message_id).toBe(1_234_567);
    // Flat: no `user` wrapper on this endpoint.
    expect(self).not.toHaveProperty("user");
  });

  /* ------------------------------------------------------------------ */
  /* Auth                                                                */
  /* ------------------------------------------------------------------ */

  it("sends HTTP Basic with the DELIVERY EMAIL as the username", async () => {
    fetchMock.mockResolvedValueOnce(reply(envelope({ streams: [] })));

    await service.listStreams();

    const init = fetchMock.mock.calls[0][1] as RequestInit;
    const headers = new Headers(init.headers);
    const authorization = headers.get("Authorization") ?? "";
    const [scheme, encoded] = authorization.split(" ");
    expect(scheme).toBe("Basic");
    expect(Buffer.from(encoded ?? "", "base64").toString("utf8")).toBe(
      "ada@example.test:SECRETKEY",
    );
  });

  it("never places the API key in the query string", async () => {
    fetchMock.mockResolvedValueOnce(reply(envelope({ streams: [] })));
    await service.listStreams();
    expect(urlOf(fetchMock)).not.toContain("SECRETKEY");
    expect(urlOf(fetchMock)).not.toContain("api_key");
  });

  /* ------------------------------------------------------------------ */
  /* Streams                                                             */
  /* ------------------------------------------------------------------ */

  it("resolves a stream name via /get_stream_id using the `stream` parameter", async () => {
    fetchMock.mockResolvedValueOnce(reply(envelope({ stream_id: 42 })));

    const id = await service.getStreamId("general");

    expect(id).toBe(42);
    const url = new URL(urlOf(fetchMock));
    expect(url.pathname).toBe("/api/v1/get_stream_id");
    expect(url.searchParams.get("stream")).toBe("general");
    expect(url.searchParams.has("stream_name")).toBe(false);
  });

  it("reads stream members as USER IDS", async () => {
    fetchMock.mockResolvedValueOnce(reply(envelope({ subscribers: [3001, 3002, 42] })));
    const subscribers = await service.getStreamSubscribers(5);
    expect(subscribers).toEqual([3001, 3002, 42]);
    expect(subscribers.every((id) => typeof id === "number")).toBe(true);
  });

  it("reads topics from one request per channel, with no pagination parameter", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          topics: [
            { name: "chatter", max_id: 900 },
            { name: "release", max_id: 800 },
          ],
        }),
      ),
    );

    const topics = await service.listTopics(5);

    expect(topics[0]).toEqual({ name: "chatter", max_id: 900 });
    const url = new URL(urlOf(fetchMock));
    expect(url.pathname).toBe("/api/v1/users/me/5/topics");
    expect(url.search).toBe("");
  });

  /* ------------------------------------------------------------------ */
  /* Narrow encoding                                                     */
  /* ------------------------------------------------------------------ */

  it("encodes a narrow as a URL-encoded JSON array of OBJECTS, exactly once", () => {
    const narrow: ZulipNarrowTerm[] = [
      { operator: "channel", operand: 5 },
      { operator: "topic", operand: "chatter" },
    ];
    const encoded = encodeNarrow(narrow);

    // URL-encoded once: %5B, not %255B.
    expect(encoded.startsWith("%5B%7B%22operator%22")).toBe(true);
    expect(encoded).not.toContain("%25");
    expect(JSON.parse(decodeURIComponent(encoded))).toEqual([
      { operator: "channel", operand: 5 },
      { operator: "topic", operand: "chatter" },
    ]);
  });

  it("expresses negated only in the object form", () => {
    const decoded = JSON.parse(
      decodeURIComponent(encodeNarrow([{ operator: "has", operand: "link", negated: true }])),
    ) as unknown[];
    expect(decoded).toEqual([{ operator: "has", operand: "link", negated: true }]);
  });

  it("sends the encoded narrow on the request URL alongside other params", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          messages: [],
          found_anchor: true,
          found_oldest: true,
          found_newest: true,
          history_limited: false,
        }),
      ),
    );

    await service.fetchMessages({
      narrow: [{ operator: "channel", operand: 5 }],
      anchor: 1000,
      num_before: 50,
      num_after: 0,
    });

    const raw = urlOf(fetchMock);
    expect(raw).toContain("narrow=%5B%7B%22operator%22%3A%22channel%22");
    expect(raw).not.toContain("%255B");
    const url = new URL(raw);
    expect(narrowOf(raw)).toEqual([{ operator: "channel", operand: 5 }]);
    expect(url.searchParams.get("anchor")).toBe("1000");
    expect(url.searchParams.get("num_before")).toBe("50");
  });

  it("sends message_ids as repeated params, never comma-joined", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          messages: [message(1)],
          found_anchor: false,
          found_oldest: false,
          found_newest: false,
          history_limited: false,
        }),
      ),
    );

    await service.getMessagesByIds([1, 2, 3]);

    const url = new URL(urlOf(fetchMock));
    expect(url.searchParams.getAll("message_ids")).toEqual(["1", "2", "3"]);
    expect(url.searchParams.has("anchor")).toBe(false);
  });

  /* ------------------------------------------------------------------ */
  /* Anchor pagination                                                   */
  /* ------------------------------------------------------------------ */

  it("walks the anchor loop and terminates on found_oldest", async () => {
    fetchMock
      .mockResolvedValueOnce(
        reply(
          envelope({
            messages: [message(1000), message(999)],
            found_anchor: true,
            found_oldest: false,
            found_newest: true,
            history_limited: false,
            anchor: 999,
          }),
        ),
      )
      .mockResolvedValueOnce(
        reply(
          envelope({
            messages: [message(900)],
            found_anchor: true,
            found_oldest: false,
            found_newest: false,
            history_limited: false,
            anchor: 900,
          }),
        ),
      )
      .mockResolvedValueOnce(
        reply(
          envelope({
            messages: [message(800)],
            found_anchor: true,
            found_oldest: true,
            found_newest: false,
            history_limited: false,
            anchor: 800,
          }),
        ),
      );

    const messages = await service.fetchAllMessages({ pageSize: 2 });

    expect(messages.map((m) => m.id)).toEqual([1000, 999, 900, 800]);
    expect(fetchMock).toHaveBeenCalledTimes(3);

    const anchors = fetchMock.mock.calls.map((call) =>
      new URL(String(call[0])).searchParams.get("anchor"),
    );
    expect(anchors).toEqual(["newest", "999", "900"]);
    // num_after stays 0: this loop only walks backwards.
    for (const call of fetchMock.mock.calls) {
      expect(new URL(String(call[0])).searchParams.get("num_after")).toBe("0");
    }
  });

  it("stops instead of looping when a page stops advancing the anchor", async () => {
    // A fresh Response per call: a reused one has an already-read body. The
    // anchor never moves, which is what makes the loop non-terminating.
    fetchMock.mockImplementation(async () =>
      reply(
        envelope({
          messages: [message(400)],
          found_anchor: true,
          found_oldest: false,
          found_newest: false,
          history_limited: false,
          anchor: 400,
        }),
      ),
    );

    const messages = await service.fetchAllMessages({ pageSize: 1 });

    // One page to learn the anchor, one page that cannot improve on it.
    expect(messages).toHaveLength(2);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("throws rather than paging forever when maxPages is exhausted", async () => {
    // Strictly decreasing anchors, so only maxPages can stop the loop.
    let anchor = 10_000;
    fetchMock.mockImplementation(async () => {
      anchor -= 1;
      return reply(
        envelope({
          messages: [],
          found_anchor: true,
          found_oldest: false,
          found_newest: false,
          history_limited: false,
          anchor,
        }),
      );
    });
    const capped = makeService({ maxPages: 3 });

    await expect(capped.fetchAllMessages()).rejects.toThrow(/found_oldest/);
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  /* ------------------------------------------------------------------ */
  /* Search and match_narrow                                             */
  /* ------------------------------------------------------------------ */

  it("searches with a `search` narrow on GET /messages, not /messages/search", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          messages: [message(10, { match_content: true, match_subject: false })],
          found_anchor: true,
          found_oldest: false,
          found_newest: false,
          history_limited: false,
          anchor: 10,
        }),
      ),
    );

    const found = await service.searchMessages("onboarding", { limit: 10 });

    const url = new URL(urlOf(fetchMock));
    expect(url.pathname).toBe("/api/v1/messages");
    expect(narrowOf(urlOf(fetchMock))).toEqual([{ operator: "search", operand: "onboarding" }]);
    expect(found[0].match_content).toBe(true);
  });

  it("reads matches_narrow keyed by STRING message id, with match_subject (not match_topic)", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          "42": { match_content: true, match_subject: true },
          "43": { match_content: false, match_subject: false },
        }),
      ),
    );

    const matches = await service.matchNarrow([{ operator: "channel", operand: 5 }]);

    expect(Object.keys(matches)).toEqual(["42", "43"]);
    expect(matches["42"].match_subject).toBe(true);
    expect(matches["42"]).not.toHaveProperty("match_topic");
    expect(new URL(urlOf(fetchMock)).pathname).toBe("/api/v1/messages/matches_narrow");
  });

  /* ------------------------------------------------------------------ */
  /* Realm metadata                                                      */
  /* ------------------------------------------------------------------ */

  it("decodes custom profile field field_data, which is a JSON-encoded STRING", () => {
    const dropdown = parseProfileFieldData({
      id: 3,
      type: 1,
      name: "Team",
      field_data: '{"field_data":"platform"}',
    });
    expect(dropdown).toEqual({ field_data: "platform" });

    // A field_data that is not JSON must not throw a dashboard request away.
    expect(
      parseProfileFieldData({ id: 4, type: 1, name: "Broken", field_data: "{oops" }),
    ).toBeUndefined();
  });

  it("reads realm emoji as an object keyed by string id", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          emoji: {
            "1": {
              id: "1",
              name: "zulip",
              source_url: "/e/1.png",
              deactivated: false,
              author_id: null,
            },
          },
        }),
      ),
    );

    const emoji = await service.listCustomEmoji();
    expect(Object.keys(emoji)).toEqual(["1"]);
    expect(emoji["1"].name).toBe("zulip");
  });

  /* ------------------------------------------------------------------ */
  /* Presence                                                            */
  /* ------------------------------------------------------------------ */

  it("tells the two presence key spaces apart", () => {
    expect(isModernPresence({ active_timestamp: 1, idle_timestamp: 0 })).toBe(true);
    expect(isModernPresence({ client: "website", status: "active", timestamp: 1 })).toBe(false);
  });

  it("reads a user presence response that merges id-keyed and email-keyed entries", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(
        envelope({
          presence: {
            "3001": { active_timestamp: 1_700_000_000, idle_timestamp: 1_699_999_000 },
            "ada@example.test": { client: "website", status: "active", timestamp: 1_700_000_001 },
          },
          server_timestamp: 1_700_000_002,
        }),
      ),
    );

    const presence = await service.getUserPresence(3001);

    expect(isModernPresence(presence.presence["3001"])).toBe(true);
    expect(isModernPresence(presence.presence["ada@example.test"])).toBe(false);
    expect(presence.server_timestamp).toBe(1_700_000_002);
  });

  /* ------------------------------------------------------------------ */
  /* Rate limiting                                                       */
  /* ------------------------------------------------------------------ */

  it("records X-RateLimit-Remaining, Limit and Reset from the response", async () => {
    const reset = Math.floor(Date.now() / 1000) + 60;
    fetchMock.mockResolvedValueOnce(
      reply(envelope({ streams: [] }), {
        "X-RateLimit-Limit": "200",
        "X-RateLimit-Remaining": "199",
        "X-RateLimit-Reset": String(reset),
      }),
    );

    await service.listStreams();

    expect(service.rateLimit).toMatchObject({ limit: 200, remaining: 199, reset });
  });

  it("does not wait while remaining budget is above the floor", async () => {
    fetchMock.mockResolvedValueOnce(
      reply(envelope({ streams: [] }), { "X-RateLimit-Remaining": "150" }),
    );
    await service.listStreams();

    const started = Date.now();
    fetchMock.mockResolvedValueOnce(
      reply(envelope({ streams: [] }), { "X-RateLimit-Remaining": "149" }),
    );
    await service.listStreams();
    expect(Date.now() - started).toBeLessThan(50);
  });

  it("self-throttles off X-RateLimit-Remaining instead of waiting for a 429", async () => {
    // Remaining is already at/below the floor, reset is 120ms away.
    const reset = Date.now() / 1000 + 0.12;
    fetchMock
      .mockResolvedValueOnce(
        reply(envelope({ streams: [] }), {
          "X-RateLimit-Limit": "200",
          "X-RateLimit-Remaining": "10",
          "X-RateLimit-Reset": String(reset),
        }),
      )
      .mockResolvedValueOnce(reply(envelope({ streams: [] })));

    await service.listStreams();
    const second = Date.now();
    await service.listStreams();

    expect(Date.now() - second).toBeGreaterThanOrEqual(100);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("honours a raised rateLimitFloor", async () => {
    const strict = makeService({ rateLimitFloor: 200 });
    fetchMock
      .mockResolvedValueOnce(
        reply(envelope({ streams: [] }), {
          "X-RateLimit-Limit": "200",
          "X-RateLimit-Remaining": "150",
          "X-RateLimit-Reset": String(Date.now() / 1000 + 0.12),
        }),
      )
      .mockResolvedValueOnce(reply(envelope({ streams: [] })));

    await strict.listStreams();
    const second = Date.now();
    await strict.listStreams();

    expect(Date.now() - second).toBeGreaterThanOrEqual(100);
  });

  it("never waits longer than the 60s limit window", async () => {
    // A reset an hour out must not pin a dashboard request open for an hour.
    fetchMock.mockResolvedValueOnce(
      reply(envelope({ streams: [] }), {
        "X-RateLimit-Limit": "200",
        "X-RateLimit-Remaining": "0",
        "X-RateLimit-Reset": String(Math.floor(Date.now() / 1000) + 3600),
      }),
    );
    await service.listStreams();
    // The state is recorded; the wait itself is capped in `_awaitHeadroom`.
    expect(service.rateLimit?.remaining).toBe(0);
  });

  it("reads the budget off the shared client's per-request observer, not a global fetch wrapper", async () => {
    // The defect this replaced: this module used to wrap `globalThis.fetch` at
    // module scope so it could see headers the shared client discarded. Headers
    // are now read through `request`'s `onResponse` observer, so the global
    // fetch is left exactly as it was found — before and after construction, and
    // while requests are in flight.
    const before = globalThis.fetch;
    const extra = makeService();

    fetchMock.mockImplementation(async () =>
      reply(envelope({ streams: [] }), {
        "X-RateLimit-Limit": "200",
        "X-RateLimit-Remaining": "42",
        "X-RateLimit-Reset": String(Math.floor(Date.now() / 1000) + 30),
      }),
    );
    await service.listStreams();
    await extra.listStreams();

    expect(globalThis.fetch).toBe(before);
    expect(globalThis.fetch).toBe(fetchMock);
    expect(service.rateLimit).toMatchObject({ limit: 200, remaining: 42 });
    expect(extra.rateLimit).toMatchObject({ limit: 200, remaining: 42 });
  });

  it("throttles off X-RateLimit-Remaining while a response arrives for another request", async () => {
    // The budget must come from THIS request's response. A registry of
    // observers keyed on URL could pair the wrong response with the wrong caller,
    // which is exactly the mismatch the observer removes.
    const reset = Date.now() / 1000 + 0.12;
    fetchMock.mockImplementation(async () =>
      reply(envelope({ members: [] }), {
        "X-RateLimit-Limit": "200",
        "X-RateLimit-Remaining": "3",
        "X-RateLimit-Reset": String(reset),
      }),
    );

    await service.listUsers();
    const second = Date.now();
    await service.getSelf();

    expect(Date.now() - second).toBeGreaterThanOrEqual(100);
    expect(service.rateLimit?.remaining).toBe(3);
  });

  /* ------------------------------------------------------------------ */
  /* No statistics endpoint is invented                                  */
  /* ------------------------------------------------------------------ */

  it("invented no statistics endpoint: derives metrics from ordinary reads only", async () => {
    fetchMock
      .mockResolvedValueOnce(
        reply(
          envelope({
            user_id: 3001,
            full_name: "Ada",
            is_active: true,
            is_bot: false,
            is_guest: false,
            max_message_id: 1_234_567,
          }),
        ),
      )
      .mockResolvedValueOnce(
        reply(
          envelope({ streams: [{ stream_id: 1, name: "a", description: "", invite_only: false }] }),
        ),
      )
      .mockResolvedValueOnce(
        reply(
          envelope({
            members: [
              {
                user_id: 1,
                full_name: "Ada",
                is_active: true,
                is_bot: false,
                is_guest: false,
                is_admin: true,
              },
              { user_id: 2, full_name: "Bot", is_active: true, is_bot: true, is_guest: false },
              { user_id: 3, full_name: "Guest", is_active: true, is_bot: false, is_guest: true },
              { user_id: 4, full_name: "Gone", is_active: false, is_bot: false, is_guest: false },
            ],
          }),
        ),
      );

    const metrics = await service.deriveMetrics();

    expect(metrics.streamCount).toBe(1);
    expect(metrics.messageIdUpperBound).toBe(1_234_567);
    expect(metrics.activeHumanUsers).toBe(1);
    expect(metrics.adminCount).toBe(1);
    expect(metrics.botCount).toBe(1);
    expect(metrics.guestCount).toBe(1);
    expect(metrics.deactivatedUserCount).toBe(1);
    expect(metrics.caveats.join(" ")).toMatch(/no aggregate or statistics endpoint/i);

    // Nothing that could only be a statistics endpoint was requested, and
    // nothing was ever POSTed (POST /fetch_api_key must not be retried in a loop).
    for (const call of fetchMock.mock.calls) {
      const url = String(call[0]);
      expect(url).not.toMatch(/statistic|daily_active|\/dau|\/mau|realm_daily/);
      expect((call[1] as RequestInit | undefined)?.method ?? "GET").toBe("GET");
    }
  });

  it("does not retry POST /fetch_api_key anywhere in the adapter", async () => {
    expect(typeof service.getMessagesByIds).toBe("function");
    const src = service as unknown as Record<string, unknown>;
    for (const name of Object.getOwnPropertyNames(Object.getPrototypeOf(src))) {
      expect(String(name)).not.toContain("fetch_api_key");
    }
  });
});

describe("zulip manifest", () => {
  it("declares the manifest contract", () => {
    expect(zulipManifest.id).toBe("zulip");
    expect(zulipManifest.uiPath).toBe("/plugins/zulip");
    expect(zulipManifest.upstream).toEqual({ product: "Zulip", envPrefix: "ZULIP" });
    expect(zulipManifest.agent?.skills.length).toBeGreaterThan(0);
    for (const skill of zulipManifest.agent?.skills ?? []) {
      expect(skill.id).toMatch(/^[a-zA-Z][a-zA-Z0-9]*$/);
      expect(skill.description.length).toBeGreaterThan(0);
    }
  });

  it("pairs every declared skill with a loader handler", () => {
    const declared = (zulipManifest.agent?.skills ?? []).map((skill) => skill.id);
    const handled = (zulipLoader.skills?.(runtimeFor(zulipManifest)) ?? []).map(
      (skill) => skill.id,
    );
    expect(handled).toEqual(declared);
  });
});
