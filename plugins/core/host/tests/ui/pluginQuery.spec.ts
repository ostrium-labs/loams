/**
 * The query layer: optimistic toggle and its rollback.
 *
 * Driven against a real `QueryClient` rather than a rendered component. The
 * workspace has no DOM (`jsdom`/`happy-dom` are not installed and the root
 * `vitest.config.ts` is not ours to edit), and more to the point the behaviour
 * under test is *cache state*, not markup -- so asserting on the cache is both
 * possible here and a more direct claim than "the switch moved".
 */
import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vite-plus/test";
import {
  PLUGINS_QUERY_KEY,
  applyOptimisticToggle,
  readPlugins,
  rollbackToggle,
} from "../../src/ui/pluginQuery.js";
import { dashboardPlugin, plugin, zulipPlugin } from "./fixtures.js";

function clientWith(...plugins: ReturnType<typeof zulipPlugin>[]) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  client.setQueryData(PLUGINS_QUERY_KEY, plugins);
  return client;
}

const enabled = (client: QueryClient, id: string) =>
  readPlugins(client)?.find((entry) => entry.id === id);

describe("PLUGINS_QUERY_KEY", () => {
  it("is the single tuple the whole app reads", () => {
    // Two different keys here would mean two caches and a silent divergence
    // between the console and the plugins page.
    expect(PLUGINS_QUERY_KEY).toEqual(["plugins"]);
  });
});

describe("applyOptimisticToggle", () => {
  it("writes the intent before the server has answered", async () => {
    const client = clientWith(zulipPlugin({ enabled: false, state: "unloaded" }));

    const context = await applyOptimisticToggle(client, { id: "zulip", enabled: true });

    // Already on, and already reported as loaded, without a request.
    expect(enabled(client, "zulip")).toMatchObject({ enabled: true, state: "loaded" });
    expect(context.previous?.[0]).toMatchObject({ enabled: false, state: "unloaded" });
  });

  it("clears a stale error when turning a plugin on", async () => {
    const client = clientWith(zulipPlugin({ enabled: false, state: "error", error: "boom" }));
    await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    expect(enabled(client, "zulip")?.error).toBeUndefined();
  });

  it("leaves every other plugin untouched", async () => {
    const client = clientWith(
      dashboardPlugin(),
      zulipPlugin({ enabled: false, state: "unloaded" }),
      plugin({ id: "forgejo", enabled: false }),
    );
    await applyOptimisticToggle(client, { id: "zulip", enabled: true });

    expect(enabled(client, "dashboard")).toMatchObject({ enabled: true, state: "loaded" });
    expect(enabled(client, "forgejo")).toMatchObject({ enabled: false });
  });

  it("records an undefined snapshot when the cache is cold", async () => {
    const client = new QueryClient();
    const context = await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    expect(context.previous).toBeUndefined();
    // Nothing to patch, so nothing is invented.
    expect(readPlugins(client)).toBeUndefined();
  });

  it("does not mutate the list it was handed", async () => {
    const client = clientWith(zulipPlugin({ enabled: false, state: "unloaded" }));
    const before = readPlugins(client);
    const snapshot = JSON.parse(JSON.stringify(before));

    await applyOptimisticToggle(client, { id: "zulip", enabled: true });

    expect(before).toEqual(snapshot);
  });

  it("drops a plugin back to unloaded when it is turned off", async () => {
    // Both halves matter: a switch reading "Off" next to a row still labelled
    // "Loaded" contradicts itself for as long as the request is in flight.
    const client = clientWith(zulipPlugin({ enabled: true, state: "loaded" }));

    await applyOptimisticToggle(client, { id: "zulip", enabled: false });

    expect(enabled(client, "zulip")).toMatchObject({ enabled: false, state: "unloaded" });
  });

  it("keeps every other entry the very same object", async () => {
    const client = clientWith(
      dashboardPlugin(),
      zulipPlugin({ enabled: false, state: "unloaded" }),
      plugin({ id: "forgejo", enabled: false }),
    );
    const before = readPlugins(client)!;

    await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    const after = readPlugins(client)!;

    // Identity, not just equality: the rows are memo-friendly on purpose, and a
    // copy per toggle would re-render every row on the page.
    expect(after[0]).toBe(before[0]);
    expect(after[2]).toBe(before[2]);
    expect(after[1]).not.toBe(before[1]);
  });
});

describe("rollbackToggle", () => {
  it("restores exactly what the cache held before the toggle", async () => {
    const client = clientWith(zulipPlugin({ enabled: false, state: "unloaded" }));
    const before = JSON.parse(JSON.stringify(readPlugins(client)));

    const context = await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    expect(enabled(client, "zulip")?.enabled).toBe(true);

    rollbackToggle(client, context);

    expect(readPlugins(client)).toEqual(before);
  });

  it("undoes a failed turn-ON as well as a failed turn-off", async () => {
    const client = clientWith(zulipPlugin({ enabled: false, state: "unloaded" }));

    const context = await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    rollbackToggle(client, context);

    expect(enabled(client, "zulip")).toMatchObject({ enabled: false, state: "unloaded" });
  });

  it("is safe with no context at all", async () => {
    const client = clientWith(zulipPlugin());
    const before = readPlugins(client);
    expect(() => rollbackToggle(client, undefined)).not.toThrow();
    expect(readPlugins(client)).toBe(before);
  });

  it("drops the entry rather than leaving a cold-start lie behind", async () => {
    const client = new QueryClient();
    const context = await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    rollbackToggle(client, context);
    expect(readPlugins(client)).toBeUndefined();
  });

  it("survives a second toggle landing in between", async () => {
    // The rollback restores the snapshot rather than patching one entry into the
    // current list, so a concurrent change to a *different* plugin is also
    // undone -- which is the safe direction: the refetch that follows decides.
    const client = clientWith(
      zulipPlugin({ enabled: false, state: "unloaded" }),
      plugin({ id: "forgejo", enabled: false }),
    );

    const context = await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    await applyOptimisticToggle(client, { id: "forgejo", enabled: true });

    rollbackToggle(client, context);

    expect(enabled(client, "zulip")?.enabled).toBe(false);
    expect(enabled(client, "forgejo")?.enabled).toBe(false);
  });
});

describe("invalidation after a toggle", () => {
  it("re-reads the list so state and changedAt are the server's, not ours", async () => {
    // Stands in for the server: the enable endpoint answered, then the list
    // read disagrees with what the UI optimistically assumed.
    const queryFn = vi
      .fn()
      .mockResolvedValue([
        zulipPlugin({ enabled: true, state: "error", error: "upstream unreachable" }),
      ]);

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(PLUGINS_QUERY_KEY, [zulipPlugin({ enabled: false, state: "unloaded" })]);

    await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    // Optimistic: on and loaded.
    expect(enabled(client, "zulip")).toMatchObject({ enabled: true, state: "loaded" });

    // What `onSettled` triggers. `staleTime: 0` so it really goes to the wire.
    await client.fetchQuery({ queryKey: PLUGINS_QUERY_KEY, queryFn, staleTime: 0 });

    // The server's real answer replaced the optimistic guess: it loaded, then
    // it failed. The UI must show that, not "on and fine".
    expect(enabled(client, "zulip")).toMatchObject({
      enabled: true,
      state: "error",
      error: "upstream unreachable",
    });
    expect(queryFn).toHaveBeenCalledTimes(1);
  });

  it("re-reads after a rollback too, so the cache cannot drift", async () => {
    const queryFn = vi.fn().mockResolvedValue([zulipPlugin({ enabled: false, state: "unloaded" })]);

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(PLUGINS_QUERY_KEY, [zulipPlugin({ enabled: false, state: "unloaded" })]);

    const context = await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    rollbackToggle(client, context);
    await client.fetchQuery({ queryKey: PLUGINS_QUERY_KEY, queryFn, staleTime: 0 });

    expect(enabled(client, "zulip")).toMatchObject({ enabled: false, state: "unloaded" });
    expect(queryFn).toHaveBeenCalledTimes(1);
  });

  it("adopts the server's entry wholesale, changedAt included", async () => {
    // There is no per-entry merge: the refetched list replaces the cache, so
    // anything the server says about the plugin -- including fields the UI
    // never writes -- reaches the row untouched.
    const queryFn = vi
      .fn()
      .mockResolvedValue([zulipPlugin({ enabled: true, state: "loaded", changedAt: 123 })]);

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(PLUGINS_QUERY_KEY, [zulipPlugin({ enabled: false, state: "unloaded" })]);

    await applyOptimisticToggle(client, { id: "zulip", enabled: true });
    // The optimistic write leaves `changedAt` alone -- the server did not move yet.
    expect(enabled(client, "zulip")?.changedAt).toBeUndefined();

    await client.fetchQuery({ queryKey: PLUGINS_QUERY_KEY, queryFn, staleTime: 0 });

    expect(enabled(client, "zulip")?.changedAt).toBe(123);
  });
});
