import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from "vite-plus/test";
import {
  UpstreamClient,
  UpstreamError,
  buildQuery,
  type QueryValue,
  type UpstreamAuth,
} from "../src/index.js";

/**
 * A `fetch` spy. Declared as `Mock<typeof fetch>` (via `vi.fn<typeof fetch>`) so
 * that `spy.mock.calls[i]` carries fetch's real parameter tuple. A spy built
 * from a zero-argument implementation types `calls` as `[][]`, which made every
 * `calls[i][1]` read below a type error even though the client really does pass
 * a `RequestInit` at runtime.
 */
type FetchSpy = Mock<typeof fetch>;

/** The `RequestInit` a fetch spy was called with. */
function initOf(spy: FetchSpy, index = 0): RequestInit {
  return spy.mock.calls[index]?.[1] ?? {};
}

/**
 * This module is the auth/query/error plumbing every adapter depends on, so its
 * bugs would surface six times over in six places. The cases below are the ones
 * that are easy to get wrong and produce confusing failures downstream:
 *
 *  - an array encoded as `a=1,2` instead of `a=1&a=2`
 *  - a `null` filter serialized as the literal text "null"
 *  - a token leaking into a URL that gets logged
 *  - a refresh hook spinning forever on a permanently-bad credential
 */

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("buildQuery", () => {
  it("emits arrays as REPEATED params, not a joined string", () => {
    // Zulip, Glitchtip and OpenPanel all read repeated params. Joining to "1,2"
    // delivers one literal value the API cannot interpret.
    expect(buildQuery({ include: [1, 2, 3] })).toBe("?include=1&include=2&include=3");
  });

  it("omits null and undefined rather than stringifying them", () => {
    expect(buildQuery({ a: null, b: undefined, c: "x" })).toBe("?c=x");
  });

  it("preserves an explicit empty array as an absent param", () => {
    expect(buildQuery({ a: [], b: "x" })).toBe("?b=x");
  });

  it("returns an empty string when there is nothing to send", () => {
    expect(buildQuery(undefined)).toBe("");
    expect(buildQuery({})).toBe("");
    expect(buildQuery({ a: null })).toBe("");
  });

  it("URL-encodes values containing reserved characters", () => {
    expect(buildQuery({ q: "a b&c=d" })).toBe("?q=a+b%26c%3Dd");
  });

  it("keeps a false boolean, which is meaningful and not an absence", () => {
    // `false` is a real filter value. Dropping it would silently invert a query.
    expect(buildQuery({ unread: false })).toBe("?unread=false");
  });
});

describe("UpstreamClient auth", () => {
  const base = "http://upstream.test";

  it("sends Bearer auth", async () => {
    const fetchSpy = vi.fn<typeof fetch>(async () => jsonResponse({ ok: true }));
    vi.stubGlobal("fetch", fetchSpy);
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "bearer", token: "t0k" } });
    await client.get("/x");
    const headers = initOf(fetchSpy, 0).headers as Headers;
    expect(headers.get("Authorization")).toBe("Bearer t0k");
  });

  it("sends Basic auth for Zulip's email:api_key form", async () => {
    const fetchSpy = vi.fn<typeof fetch>(async () => jsonResponse({}));
    vi.stubGlobal("fetch", fetchSpy);
    const client = new UpstreamClient({
      baseUrl: base,
      auth: { kind: "basic", username: "me@example.com", password: "abc123" },
    });
    await client.get("/api/v1/users");
    const headers = initOf(fetchSpy, 0).headers as Headers;
    const decoded = Buffer.from(
      headers.get("Authorization")!.replace("Basic ", ""),
      "base64",
    ).toString();
    expect(decoded).toBe("me@example.com:abc123");
  });

  it("sends a raw Authorization value for Forgejo's 'token <t>' scheme", async () => {
    const fetchSpy = vi.fn<typeof fetch>(async () => jsonResponse({}));
    vi.stubGlobal("fetch", fetchSpy);
    const client = new UpstreamClient({
      baseUrl: base,
      auth: { kind: "authorization-raw", value: "token sha1:abc" },
    });
    await client.get("/api/v1/repos/search");
    const headers = initOf(fetchSpy, 0).headers as Headers;
    expect(headers.get("Authorization")).toBe("token sha1:abc");
  });

  it("puts a query-token credential in the URL, not the headers", async () => {
    const fetchSpy = vi.fn<typeof fetch>(async () => jsonResponse({}));
    vi.stubGlobal("fetch", fetchSpy);
    const client = new UpstreamClient({
      baseUrl: base,
      auth: { kind: "query-token", param: "token_auth", token: "secret" },
    });
    await client.get("/index.php", { module: "API", method: "VisitsSummary.get" });
    const url = String(fetchSpy.mock.calls[0]![0]);
    expect(url).toContain("token_auth=secret");
    const headers = initOf(fetchSpy, 0).headers as Headers;
    expect(headers.get("Authorization")).toBeNull();
  });

  it("keeps the query-token out of resolve() so it cannot leak into a log line", () => {
    const client = new UpstreamClient({
      baseUrl: base,
      auth: { kind: "query-token", param: "api_key", token: "secret" },
    });
    // `resolve` is what an adapter would put in a warning message.
    expect(client.resolve("/index.php", { module: "API" })).not.toContain("secret");
  });

  it("lets per-call headers override the configured auth", async () => {
    const fetchSpy = vi.fn<typeof fetch>(async () => jsonResponse({}));
    vi.stubGlobal("fetch", fetchSpy);
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "bearer", token: "a" } });
    await client.request("GET", "/x", { headers: { Authorization: "Bearer b" } });
    const headers = initOf(fetchSpy, 0).headers as Headers;
    expect(headers.get("Authorization")).toBe("Bearer b");
  });
});

describe("UpstreamClient errors", () => {
  const base = "http://upstream.test";

  afterEach(() => vi.unstubAllGlobals());

  it("raises UpstreamError carrying status, url and a truncated body", async () => {
    vi.stubGlobal("fetch", async () => new Response("Invalid API key", { status: 403 }));
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });
    const err = await client.get("/x").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(UpstreamError);
    expect((err as UpstreamError).status).toBe(403);
    expect((err as UpstreamError).body).toContain("Invalid API key");
    expect((err as UpstreamError).isAuthFailure).toBe(true);
  });

  it("classifies a network failure as status 0, not as a crash", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("fetch failed");
    });
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });
    const err = await client.get("/x").catch((e: unknown) => e);
    expect((err as UpstreamError).status).toBe(0);
  });

  it("reports a timeout as 408 rather than a generic fetch failure", async () => {
    // A slow upstream and an absent one need different remedies.
    vi.stubGlobal("fetch", async () => {
      const err = new Error("The operation was aborted");
      err.name = "AbortError";
      throw err;
    });
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" }, timeoutMs: 5 });
    const err = await client.get("/x").catch((e: unknown) => e);
    expect((err as UpstreamError).status).toBe(408);
    expect((err as UpstreamError).body).toContain("timed out");
  });

  it("reports a 2xx non-JSON body as an upstream error with the text", async () => {
    vi.stubGlobal("fetch", async () => new Response("<html>login</html>", { status: 200 }));
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });
    const err = await client.get("/x").catch((e: unknown) => e);
    expect((err as UpstreamError).body).toContain("non-JSON body");
  });

  it("returns undefined for a 2xx with an empty body", async () => {
    vi.stubGlobal("fetch", async () => new Response(null, { status: 204 }));
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });
    await expect(client.get("/x")).resolves.toBeUndefined();
  });
});

describe("UpstreamClient credential refresh", () => {
  const base = "http://upstream.test";

  afterEach(() => vi.unstubAllGlobals());

  it("refreshes once on 401 and replays the request", async () => {
    let calls = 0;
    vi.stubGlobal("fetch", async () => {
      calls += 1;
      return calls === 1 ? new Response("expired", { status: 401 }) : jsonResponse({ ok: true });
    });
    const refresh = vi.fn(async () => ({ kind: "bearer", token: "fresh" }) as UpstreamAuth);
    const client = new UpstreamClient({
      baseUrl: base,
      auth: { kind: "bearer", token: "stale" },
      refresh,
    });

    await expect(client.get("/x")).resolves.toEqual({ ok: true });
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("does not refresh when there is no hook", async () => {
    vi.stubGlobal("fetch", async () => new Response("nope", { status: 401 }));
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "bearer", token: "x" } });
    await expect(client.get("/x")).rejects.toBeInstanceOf(UpstreamError);
  });

  it("gives up after one refresh instead of spinning", async () => {
    // A permanently-bad credential must not retry forever on every dashboard poll.
    let calls = 0;
    vi.stubGlobal("fetch", async () => {
      calls += 1;
      return new Response("nope", { status: 401 });
    });
    const refresh = vi.fn(async () => ({ kind: "bearer", token: "also-bad" }) as UpstreamAuth);
    const client = new UpstreamClient({
      baseUrl: base,
      auth: { kind: "bearer", token: "x" },
      refresh,
    });

    await expect(client.get("/x")).rejects.toBeInstanceOf(UpstreamError);
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(calls).toBe(2); // original + one replay
  });

  it("propagates the original error when refresh itself throws", async () => {
    vi.stubGlobal("fetch", async () => new Response("nope", { status: 403 }));
    const refresh = vi.fn(async () => {
      throw new Error("login endpoint unreachable");
    });
    const warns: string[] = [];
    const client = new UpstreamClient(
      { baseUrl: base, auth: { kind: "bearer", token: "x" }, refresh },
      { debug: () => {}, info: () => {}, warn: (m: unknown) => warns.push(String(m)) },
    );
    await expect(client.get("/x")).rejects.toBeInstanceOf(UpstreamError);
    expect(warns.join("\n")).toContain("login endpoint unreachable");
  });

  it("logs a repeated refresh failure only once", async () => {
    vi.stubGlobal("fetch", async () => new Response("nope", { status: 403 }));
    const refresh = vi.fn(async () => {
      throw new Error("boom");
    });
    const warns: string[] = [];
    const client = new UpstreamClient(
      { baseUrl: base, auth: { kind: "bearer", token: "x" }, refresh },
      { debug: () => {}, info: () => {}, warn: (m: unknown) => warns.push(String(m)) },
    );
    await client.get("/x").catch(() => {});
    await client.get("/x").catch(() => {});
    await client.get("/x").catch(() => {});
    expect(refresh).toHaveBeenCalledTimes(3);
    expect(warns).toHaveLength(1);
  });
});

describe("UpstreamClient response observer", () => {
  const base = "http://upstream.test";

  afterEach(() => vi.unstubAllGlobals());

  it("hands over the live Response with its headers, before the body is consumed", async () => {
    // The header is the whole reason the hook exists: Zulip throttles off
    // X-RateLimit-Remaining, Forgejo totals off X-Total-Count, GlitchTip paginates
    // off X-Hits/X-Max-Hits/Link — none of which the parsed body contains.
    vi.stubGlobal(
      "fetch",
      async () =>
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json", "X-RateLimit-Remaining": "199" },
        }),
    );
    const seen: Array<{ remaining: string | null; bodyUsed: boolean; url: string }> = [];
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });

    const body = await client.request<{ ok: boolean }>("GET", "/api/v1/streams", {
      onResponse: (res, meta) => {
        seen.push({
          remaining: res.headers.get("X-RateLimit-Remaining"),
          bodyUsed: res.bodyUsed,
          url: meta.url,
        });
      },
    });

    expect(body).toEqual({ ok: true });
    expect(seen).toEqual([
      { remaining: "199", bodyUsed: false, url: "http://upstream.test/api/v1/streams" },
    ]);
  });

  it("fires once per response, including for a non-2xx the caller then fails on", async () => {
    vi.stubGlobal(
      "fetch",
      async () => new Response(JSON.stringify({ message: "Not Found" }), { status: 404 }),
    );
    const attempts: number[] = [];
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });

    await expect(
      client.request("GET", "/x", {
        onResponse: (_res, meta) => attempts.push(meta.attempt),
      }),
    ).rejects.toBeInstanceOf(UpstreamError);

    // The error path still observed it: a 404's headers are still headers.
    expect(attempts).toEqual([1]);
  });

  it("observes the intermediate 401 AND the replay, and `attempt` tells them apart", async () => {
    let calls = 0;
    vi.stubGlobal("fetch", async () => {
      calls += 1;
      return calls === 1
        ? new Response("expired", { status: 401, headers: { "X-Attempt": "stale" } })
        : new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "Content-Type": "application/json", "X-Attempt": "fresh" },
          });
    });
    const observed: Array<{ attempt: number; status: number; marker: string | null }> = [];
    const client = new UpstreamClient({
      baseUrl: base,
      auth: { kind: "bearer", token: "stale" },
      refresh: async () => ({ kind: "bearer", token: "fresh" }),
    });

    await client.get("/x", undefined, {
      onResponse: (res, meta) =>
        observed.push({
          attempt: meta.attempt,
          status: meta.status,
          marker: res.headers.get("X-Attempt"),
        }),
    });

    // Both responses were seen, and `attempt` is what distinguishes them. An
    // observer that only wants the response whose body it will read keeps the
    // last entry; one that must ignore a pre-refresh 401 compares attempt.
    expect(observed).toEqual([
      { attempt: 1, status: 401, marker: "stale" },
      { attempt: 2, status: 200, marker: "fresh" },
    ]);
  });

  it("does not observe anything when there is no response to observe", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("fetch failed");
    });
    const onResponse = vi.fn();
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });

    await expect(client.request("GET", "/x", { onResponse })).rejects.toBeInstanceOf(UpstreamError);
    // A timeout or a DNS failure has no headers, so there is nothing to hand over.
    expect(onResponse).not.toHaveBeenCalled();
  });

  it("does not fail the request when the observer throws, and logs it once", async () => {
    // Instrumentation must never break the request it is measuring, and a broken
    // observer must not log a stack trace per dashboard poll.
    vi.stubGlobal("fetch", async () => jsonResponse({ ok: true }, 200));
    const warns: string[] = [];
    const client = new UpstreamClient(
      { baseUrl: base, auth: { kind: "none" } },
      {
        debug: () => {},
        info: () => {},
        warn: (m: unknown) => warns.push(String(m)),
      },
    );
    const onResponse = vi.fn(() => {
      throw new Error("header parse blew up");
    });

    await expect(client.get("/x", undefined, { onResponse })).resolves.toEqual({ ok: true });
    await expect(client.get("/x", undefined, { onResponse })).resolves.toEqual({ ok: true });
    await expect(client.get("/x", undefined, { onResponse })).resolves.toEqual({ ok: true });

    expect(onResponse).toHaveBeenCalledTimes(3);
    expect(warns).toHaveLength(1);
    expect(warns[0]).toContain("header parse blew up");
  });

  it("is available on get, post and put as well as request", async () => {
    vi.stubGlobal("fetch", async () => jsonResponse({ ok: true }));
    const seen: Array<[string, number]> = [];
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });
    const onResponse = (_res: Response, meta: { status: number }) => seen.push(["x", meta.status]);

    await client.get("/x", undefined, { onResponse });
    await client.post("/x", { a: 1 }, undefined, { onResponse });
    await client.put("/x", { a: 1 }, undefined, { onResponse });

    expect(seen).toEqual([
      ["x", 200],
      ["x", 200],
      ["x", 200],
    ]);
  });

  it("leaves a request with no observer byte-for-byte unchanged", async () => {
    // The hook is purely additive: omitting it must not alter the URL, the
    // headers, the parsed result or the error behaviour.
    const fetchSpy = vi.fn<typeof fetch>(async () => jsonResponse({ ok: true }));
    vi.stubGlobal("fetch", fetchSpy);
    const plain = new UpstreamClient({ baseUrl: base, auth: { kind: "bearer", token: "t" } });

    await expect(plain.get("/x", { a: 1 })).resolves.toEqual({ ok: true });
    await expect(plain.request("GET", "/x")).resolves.toEqual({ ok: true });

    expect(fetchSpy).toHaveBeenCalledTimes(2);
    expect(String(fetchSpy.mock.calls[0]![0])).toBe("http://upstream.test/x?a=1");
    expect(initOf(fetchSpy, 0).headers).toBeDefined();
  });
});

describe("UpstreamClient url building", () => {
  const base = "http://upstream.test";

  it("trims trailing slashes from the base url", () => {
    const client = new UpstreamClient({ baseUrl: "http://u.test/", auth: { kind: "none" } });
    expect(client.resolve("/api/v1/x")).toBe("http://u.test/api/v1/x");
  });

  it("adds a leading slash to a bare path", () => {
    const client = new UpstreamClient({ baseUrl: "http://u.test", auth: { kind: "none" } });
    expect(client.resolve("api/v1/x")).toBe("http://u.test/api/v1/x");
  });

  it("appends a query string already on the path", () => {
    const client = new UpstreamClient({ baseUrl: "http://u.test", auth: { kind: "none" } });
    expect(client.resolve("/index.php?a=1")).toBe("http://u.test/index.php?a=1");
  });

  it("sets a JSON content type only when there is a body", async () => {
    const fetchSpy = vi.fn<typeof fetch>(async () => jsonResponse({}));
    vi.stubGlobal("fetch", fetchSpy);
    const client = new UpstreamClient({ baseUrl: base, auth: { kind: "none" } });
    await client.get("/x");
    expect((initOf(fetchSpy, 0).headers as Headers).get("Content-Type")).toBeNull();
    await client.post("/x", { a: 1 });
    const headers = initOf(fetchSpy, 1).headers as Headers;
    expect(headers.get("Content-Type")).toBe("application/json");
    expect(initOf(fetchSpy, 1).body).toBe('{"a":1}');
    vi.unstubAllGlobals();
  });
});
