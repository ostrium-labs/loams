/**
 * The one place the shell talks to the plugin API.
 *
 * TanStack Query owns the list outright. `PluginsProvider` exists so the
 * console, the plugins page and the plugin page all read the *same* cache
 * entry instead of each starting its own fetch -- with the query key shared
 * that is automatic, and the provider is kept for two other reasons: it holds
 * the per-plugin feedback and pending maps that have no business in a query
 * cache, and it lets a page be rendered against a fixed store in tests without
 * a network or a DOM.
 */
import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { PluginStatus } from "../types.js";
import { fetchPlugins, setPluginEnabled } from "./api.js";
import { ALWAYS_ON_REASON, messageOf } from "./pluginState.js";
import { PLUGINS_QUERY_KEY, applyOptimisticToggle, rollbackToggle } from "./pluginQuery.js";
import type { ToggleContext, ToggleVariables } from "./pluginQuery.js";

export type FeedbackKind = "success" | "error";

export interface PluginFeedback {
  kind: FeedbackKind;
  message: string;
}

export interface PluginStore {
  plugins: PluginStatus[];
  loading: boolean;
  /** A failure to read the list at all. Per-plugin failures live in `feedback`. */
  error: string | null;
  /** Re-read the list. Rejects so callers (like the toggle) can react. */
  refresh: () => Promise<PluginStatus[]>;
  /** Turn a plugin on or off, optimistically and with rollback on failure. */
  setEnabled: (plugin: PluginStatus, enabled: boolean) => Promise<ToggleResult>;
  /** Ids with a toggle request in flight. */
  pending: Record<string, boolean>;
  feedback: Record<string, PluginFeedback>;
  dismissFeedback: (id: string) => void;
}

export interface ToggleResult {
  ok: boolean;
  message?: string;
}

/*
 * The client-side `alwaysOn` guard below is the one behaviour the pre-TanStack
 * implementation owned that the query layer does not: React Query has no way to
 * know a request is pointless, so refusing it without a round trip has to happen
 * here. The rest of that old implementation -- the optimistic write, its
 * rollback, and the follow-up refresh -- is `pluginQuery.ts`.
 */

/** How long a "turned on" confirmation stays up before it retires itself. */
const SUCCESS_FEEDBACK_MS = 4000;

const PluginStoreContext = createContext<PluginStore | null>(null);

/**
 * The fetching store.
 *
 * Feedback and pending live in React state rather than in the query cache:
 * they are per-interaction UI, not server state, and they must survive a
 * refetch that replaces the list.
 */
function usePluginStore(): PluginStore {
  const queryClient = useQueryClient();

  const [pending, setPending] = useState<Record<string, boolean>>({});
  const [feedback, setFeedback] = useState<Record<string, PluginFeedback>>({});
  const feedbackTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});

  const query = useQuery<PluginStatus[]>({
    queryKey: PLUGINS_QUERY_KEY,
    queryFn: fetchPlugins,
  });

  const refresh = useCallback(async (): Promise<PluginStatus[]> => {
    return queryClient.fetchQuery({
      queryKey: PLUGINS_QUERY_KEY,
      queryFn: fetchPlugins,
    });
  }, [queryClient]);

  /*
   * Confirmation timers outlive the component otherwise, and React 19 has no
   * cleanup hook for fire-and-forget notifications.
   */
  useEffect(() => {
    const timers = feedbackTimers.current;
    return () => {
      for (const timer of Object.values(timers)) clearTimeout(timer);
    };
  }, []);

  const dismissFeedback = useCallback((id: string) => {
    setFeedback((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, []);

  const report = useCallback(
    (id: string, entry: PluginFeedback) => {
      const existing = feedbackTimers.current[id];
      if (existing) clearTimeout(existing);

      setFeedback((prev) => ({ ...prev, [id]: entry }));

      if (entry.kind !== "success") return;
      feedbackTimers.current[id] = setTimeout(() => {
        delete feedbackTimers.current[id];
        dismissFeedback(id);
      }, SUCCESS_FEEDBACK_MS);
    },
    [dismissFeedback],
  );

  const toggle = useMutation<PluginStatus, Error, ToggleVariables, ToggleContext>({
    mutationFn: ({ id, enabled }) => setPluginEnabled(id, enabled),

    /*
     * Wrapped rather than passed as `onMutate: applyOptimisticToggle`.
     *
     * React Query calls `onMutate(variables, context)`, and
     * `applyOptimisticToggle` takes `(client, variables)`. Passing the function
     * directly would bind the mutation *variables* to the `QueryClient`
     * parameter -- the optimistic write would then run against a garbage client
     * and the cache would never move.
     */
    onMutate: (variables) => applyOptimisticToggle(queryClient, variables),

    onError: (error, variables, context) => {
      // The cache must not keep showing a state the server just refused.
      rollbackToggle(queryClient, context);
      report(variables.id, { kind: "error", message: messageOf(error) });
    },

    onSettled: () => {
      /*
       * Invalidate unconditionally, on success *and* on failure.
       *
       * On success this is what makes `state` and `changedAt` real: the
       * enable/disable response is only the new status, and a server that
       * degraded the load (unloaded vs error) says so on the next read.
       *
       * On failure it re-reads the list so the cache cannot drift away from
       * the server after a rollback. The rollback above has already restored
       * the last known-good snapshot; this refetch is what proves it.
       */
      void queryClient.invalidateQueries({ queryKey: PLUGINS_QUERY_KEY });
    },
  });

  const setEnabled = useCallback(
    async (plugin: PluginStatus, enabled: boolean): Promise<ToggleResult> => {
      // Client-side guard for `alwaysOn`. The server refuses too (409/400);
      // this only avoids making a request we know will fail.
      if (plugin.alwaysOn === true) {
        return { ok: false, message: ALWAYS_ON_REASON };
      }

      setPending((prev) => ({ ...prev, [plugin.id]: true }));
      try {
        await toggle.mutateAsync({ id: plugin.id, enabled });
        // `onError` has already reported the failure; reaching here means the
        // server accepted it.
        report(plugin.id, {
          kind: "success",
          message: `${plugin.name} is ${enabled ? "on" : "off"}.`,
        });
        return { ok: true };
      } catch (error) {
        return { ok: false, message: messageOf(error) };
      } finally {
        setPending((prev) => {
          if (!(plugin.id in prev)) return prev;
          const next = { ...prev };
          delete next[plugin.id];
          return next;
        });
      }
    },
    [report, toggle],
  );

  return useMemo<PluginStore>(
    () => ({
      plugins: query.data ?? [],
      loading: query.isPending,
      error: query.error ? messageOf(query.error) : null,
      refresh,
      setEnabled,
      pending,
      feedback,
      dismissFeedback,
    }),
    [
      query.data,
      query.isPending,
      query.error,
      refresh,
      setEnabled,
      pending,
      feedback,
      dismissFeedback,
    ],
  );
}

/**
 * The fetching half of the provider.
 *
 * Split out as its own component so hooks only run when a store is actually
 * wanted. `PluginsProvider` decides between "a fixed store" and "fetch" before
 * anything below is called, which is what lets a unit test mount the pages
 * without standing up a `QueryClient` at all.
 */
function FetchedPluginsProvider({ children }: { children: ReactNode }) {
  const store = usePluginStore();
  return <PluginStoreContext.Provider value={store}>{children}</PluginStoreContext.Provider>;
}

export function PluginsProvider({
  children,
  store: provided,
}: {
  children: ReactNode;
  /** Supply a fixed store instead of fetching. Used by tests. */
  store?: PluginStore;
}) {
  if (provided) {
    return <PluginStoreContext.Provider value={provided}>{children}</PluginStoreContext.Provider>;
  }
  return <FetchedPluginsProvider>{children}</FetchedPluginsProvider>;
}

export function usePlugins(): PluginStore {
  const store = useContext(PluginStoreContext);
  if (!store) throw new Error("usePlugins must be used inside a <PluginsProvider>");
  return store;
}
