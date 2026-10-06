import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import {
  ApiError,
  fetchPlugin,
  fetchPluginAgentCard,
  fetchPlugins,
  setPluginEnabled,
} from "../../src/ui/api.js";
import { plugin } from "./fixtures.js";

const realFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = realFetch;
});

function stubFetch(handler: (url: string, init?: RequestInit) => Response | Promise<Response>) {
  const spy = vi.fn((input: unknown, init?: RequestInit) => handler(String(input), init));
  globalThis.fetch = spy as unknown as typeof fetch;
  return spy;
}

function json(body: unknown, status = 200, statusText = "OK"): Response {
  return new Response(JSON.stringify(body), {
    status,
    statusText,
    headers: { "Content-Type": "application/json" },
  });
}

describe("fetchPlugins", () => {
  it("unwraps { plugins } and returns an empty list for anything else", async () => {
    stubFetch(() => json({ plugins: [plugin({ id: "zulip" })] }));
    expect((await fetchPlugins()).map((p) => p.id)).toEqual(["zulip"]);

    stubFetch(() => json({}));
    expect(await fetchPlugins()).toEqual([]);

    stubFetch(() => json([]));
    expect(await fetchPlugins()).toEqual([]);
  });

  it("surfaces the server's { error }", async () => {
    stubFetch(() => json({ error: "plugin registry unavailable" }, 503, "Service Unavailable"));
    await expect(fetchPlugins()).rejects.toThrow("plugin registry unavailable");
  });

  it("falls back to { message }", async () => {
    stubFetch(() => json({ message: "cordis context not ready" }, 500));
    await expect(fetchPlugins()).rejects.toThrow("cordis context not ready");
  });

  it("says so when the error body is not JSON", async () => {
    stubFetch(
      () =>
        new Response("<html>502 Bad Gateway</html>", { status: 502, statusText: "Bad Gateway" }),
    );
    await expect(fetchPlugins()).rejects.toThrow(/502 Bad Gateway.*not JSON/);
  });

  it("distinguishes a network failure from an HTTP failure", async () => {
    stubFetch(() => {
      throw new TypeError("fetch failed");
    });

    const error = await fetchPlugins().catch((cause: unknown) => cause);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).isNetworkError).toBe(true);
    expect((error as ApiError).status).toBe(0);
    expect((error as ApiError).message).toContain("Could not reach the server at /api");
  });

  it("rejects an empty success body rather than returning undefined", async () => {
    stubFetch(() => new Response("", { status: 200 }));
    await expect(fetchPlugins()).rejects.toThrow(/empty response/);
  });
});

describe("fetchPlugin", () => {
  it("escapes the id and returns the status", async () => {
    const spy = stubFetch(() => json(plugin({ id: "its-a-plan" })));
    const result = await fetchPlugin("its-a-plan");
    expect(spy).toHaveBeenCalledWith("/api/plugins/its-a-plan", undefined);
    expect(result.id).toBe("its-a-plan");
  });
});

describe("setPluginEnabled", () => {
  it("POSTs to enable and disable", async () => {
    const spy = stubFetch(() => json(plugin({ id: "zulip", enabled: true })));

    await setPluginEnabled("zulip", true);
    expect(spy).toHaveBeenLastCalledWith("/api/plugins/zulip/enable", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
    });

    await setPluginEnabled("zulip", false);
    expect(spy).toHaveBeenLastCalledWith("/api/plugins/zulip/disable", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
    });
  });

  it("reports a refusal with the server's own words", async () => {
    stubFetch(() => json({ error: 'plugin "dashboard" is always on' }, 409, "Conflict"));
    const error = await setPluginEnabled("dashboard", false).catch((cause: unknown) => cause);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(409);
    expect((error as ApiError).message).toBe('plugin "dashboard" is always on');
  });
});

describe("fetchPluginAgentCard", () => {
  it("reads the A2A agent card", async () => {
    const spy = stubFetch(() =>
      json({ name: "zulip-agent", description: "d", version: "1.0.0", skills: [] }),
    );
    const card = await fetchPluginAgentCard("zulip");
    expect(spy).toHaveBeenCalledWith("/api/plugins/zulip/agent", undefined);
    expect(card.name).toBe("zulip-agent");
  });
});
