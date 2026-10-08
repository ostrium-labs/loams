/*
 * The TanStack Query side of the plugin list.
 *
 * Plugin state is server state, so react-query owns it outright: one
 * `['plugins']` entry is the single cache the console, the plugins page and
 * the plugin page all read. There are deliberately **no react-router loaders**
 * here -- a loader would fetch the same list into a second cache and the two
 * would disagree after the first toggle.
 *
 * The optimistic toggle is the part most likely to be wrong, and it is the part
 * that has to be provably right, so the cache surgery lives here as plain
 * functions over a `QueryClient` rather than inline in a component. `usePlugins`
 * wires them into a `useMutation`; the tests drive them against a real client.
 */
import type { QueryClient } from "@tanstack/react-query";
import type { PluginStatus } from "../types.js";
import { optimisticToggle } from "./pluginState.js";

/**
 * The one cache key. A single-element tuple so a narrow invalidation is still
 * possible later if per-plugin detail queries appear.
 */
export const PLUGINS_QUERY_KEY = ["plugins"] as const;

export interface ToggleVariables {
  id: string;
  enabled: boolean;
}

/**
 * What `onMutate` hands to `onError` so a failure can be undone.
 *
 * The whole list is snapshotted rather than just the one entry: the optimistic
 * update is a `map` over the array, so restoring a single element is not enough
 * to prove the rollback is exact.
 */
export interface ToggleContext {
  previous: PluginStatus[] | undefined;
}

export function readPlugins(client: QueryClient): PluginStatus[] | undefined {
  return client.getQueryData<PluginStatus[]>(PLUGINS_QUERY_KEY);
}

/**
 * Write the user's intent locally, before the server has agreed to it.
 *
 * Also cancels any in-flight read. Without the cancel, a refresh that was
 * already on the wire would land *after* this write and quietly revert the
 * switch while it is still round-tripping.
 */
export async function applyOptimisticToggle(
  client: QueryClient,
  { id, enabled }: ToggleVariables,
): Promise<ToggleContext> {
  await client.cancelQueries({ queryKey: PLUGINS_QUERY_KEY });

  const previous = readPlugins(client);
  if (previous) {
    client.setQueryData<PluginStatus[]>(PLUGINS_QUERY_KEY, optimisticToggle(previous, id, enabled));
  }
  return { previous };
}

/**
 * Put the cache back the way the server last described it.
 *
 * The UI must never claim a state the server disagrees with, so this is not
 * best-effort: if the mutation failed, the optimistic value is *always*
 * replaced by the snapshot, even when the snapshot was missing -- in which
 * case the entry is dropped rather than left lying.
 *
 * Note this restores the whole list rather than patching one entry into the
 * current one. A rollback after a *failed* request has to defeat any
 * concurrent write too, and the snapshot is the only thing known to be true.
 */
export function rollbackToggle(client: QueryClient, context: ToggleContext | undefined): void {
  if (!context) return;
  if (context.previous === undefined) {
    client.removeQueries({ queryKey: PLUGINS_QUERY_KEY });
    return;
  }
  client.setQueryData<PluginStatus[]>(PLUGINS_QUERY_KEY, context.previous);
}
