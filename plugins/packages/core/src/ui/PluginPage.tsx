/**
 * The `/plugins/:id` route.
 *
 * Three things can go wrong before the plugin's own page can render -- the id
 * is missing, the id names a plugin nobody has registered, or the plugin is
 * switched off. Each gets its own state rather than a shared "not available",
 * because the fix the user needs is different in every case.
 */
import React from "react";
import { Link } from "react-router";
import type { PluginStatus } from "../types.js";
import { findPlugin } from "./pluginState.js";
import { usePlugins } from "./usePlugins.jsx";
import type { PluginStore } from "./usePlugins.jsx";
import { cn } from "./cn.js";
import {
  BTN_BASE,
  BTN_OUTLINE,
  BTN_PRIMARY,
  BTN_SM,
  EMPTY_STATE,
  EMPTY_STATE_ACTIONS,
  EMPTY_STATE_BODY,
  EMPTY_STATE_MARK,
  EMPTY_STATE_TITLE,
  PAGE,
  SKELETON_LINE,
  SKELETON_TITLE,
  SKELETON_TAIL,
} from "./uiClasses.js";
import { AlertIcon } from "./icons.js";
import { Skeleton } from "./primitives/index.js";
import { resolvePluginPage } from "./pluginPages/registry.js";

function PluginUnavailable({
  title,
  body,
  children,
}: {
  title: string;
  body: string;
  children?: React.ReactNode;
}) {
  return (
    <div className={PAGE}>
      <div className={EMPTY_STATE}>
        <div className={EMPTY_STATE_MARK} aria-hidden="true">
          <AlertIcon />
        </div>
        <h1 className={EMPTY_STATE_TITLE}>{title}</h1>
        <p className={EMPTY_STATE_BODY}>{body}</p>
        <div className={EMPTY_STATE_ACTIONS}>
          <Link to="/plugins" className={cn(BTN_BASE, BTN_OUTLINE)}>
            All plugins
          </Link>
          <Link to="/console" className={cn(BTN_BASE, BTN_PRIMARY)}>
            Open the console
          </Link>
        </div>
        {children}
      </div>
    </div>
  );
}

/**
 * The "switched off" state. Rendered by `PluginPage` with the hook it already
 * owns -- a nested `usePlugins()` here would mean a second fetch of the list.
 */
function PluginOff({ plugin, store }: { plugin: PluginStatus; store: PluginStore }) {
  const busy = Boolean(store.pending[plugin.id]);
  const note = store.feedback[plugin.id];

  return (
    <PluginUnavailable
      title={`${plugin.name} is turned off`}
      body={`${plugin.description} Its routes and agent skills are disposed while it is off.`}
    >
      <button
        type="button"
        className={cn(BTN_BASE, BTN_PRIMARY)}
        disabled={busy}
        aria-busy={busy || undefined}
        onClick={() => void store.setEnabled(plugin, true)}
      >
        {busy ? "Turning on…" : `Turn on ${plugin.name}`}
      </button>
      {note && (
        <p
          className={cn(
            "inline-flex items-center gap-[0.4rem] rounded border py-[0.2rem] px-[0.5rem] text-[0.76rem] leading-[1.4]",
            note.kind === "error"
              ? "border-[color-mix(in_srgb,var(--danger)_28%,var(--bg-card))] bg-[color-mix(in_srgb,var(--danger)_7%,var(--bg-card))] text-danger"
              : "border-[color-mix(in_srgb,var(--success)_28%,var(--bg-card))] bg-[color-mix(in_srgb,var(--success)_8%,var(--bg-card))] text-success",
          )}
          role="status"
        >
          {note.message}
        </p>
      )}
    </PluginUnavailable>
  );
}

export function PluginPage({ id }: { id: string | undefined }) {
  const store = usePlugins();
  const { plugins, loading, error } = store;
  const plugin = findPlugin(plugins, id);

  if (loading && plugins.length === 0) {
    return (
      <div className={PAGE}>
        <Skeleton className={SKELETON_TITLE} />
        <Skeleton className={SKELETON_LINE} />
        <Skeleton className={SKELETON_TAIL} />
      </div>
    );
  }

  if (!id) {
    return (
      <PluginUnavailable
        title="No plugin selected"
        body="That URL does not name a plugin. Pick one from the list of plugins that are switched on."
      />
    );
  }

  if (!plugin) {
    return (
      <PluginUnavailable
        title={`No plugin called “${id}”`}
        // Say why we might not know: a failed list read makes an existing
        // plugin look exactly like a missing one.
        body={
          error
            ? `The plugin list could not be read, so this page cannot confirm whether “${id}” exists. ${error}`
            : `Nothing is registered under the id “${id}”. It may have been uninstalled, or the link may be out of date.`
        }
      >
        <button
          type="button"
          className={cn(BTN_BASE, BTN_OUTLINE, BTN_SM)}
          onClick={() => void store.refresh().catch(() => undefined)}
        >
          Retry
        </button>
      </PluginUnavailable>
    );
  }

  if (!plugin.enabled) {
    return <PluginOff plugin={plugin} store={store} />;
  }

  const Page = resolvePluginPage(plugin);
  return <Page plugin={plugin} />;
}
