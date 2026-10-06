/**
 * The plugin console: every plugin the host knows about, with a switch.
 *
 * The list arrives pre-sorted from the server (always-on first, then `order`,
 * then name) and is rendered in exactly that order -- duplicating the sort here
 * would give two places for the rule to drift.
 */
import React from "react";
import { Link } from "react-router";
import type { PluginStatus } from "../types.js";
import {
  ALWAYS_ON_REASON,
  countSkills,
  describeState,
  isAlwaysOn,
  partitionPlugins,
  pluginHref,
} from "./pluginState.js";
import type { PluginFeedback } from "./usePlugins.jsx";
import { usePlugins } from "./usePlugins.jsx";
import { cn } from "./cn.js";
import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
  Badge,
  Skeleton,
  Switch,
} from "./primitives/index.js";
import {
  BTN_BASE,
  BTN_OUTLINE,
  BTN_SM,
  PAGE,
  PAGE_COUNT,
  PAGE_HEAD,
  PAGE_HEAD_ACTIONS,
  PAGE_HEAD_TEXT,
  PAGE_SUBTITLE,
  PAGE_TITLE,
} from "./uiClasses.js";
import { AlertIcon, ArrowRightIcon, LayersIcon, PlugIcon } from "./icons.js";

function titleId(plugin: PluginStatus): string {
  return `plugin-console-${plugin.id}-title`;
}

/* ------------------------------------------------------------- fragments */

const GROUP = "flex flex-col gap-[0.6rem]";

const GROUP_HEAD = "flex flex-wrap items-baseline gap-3 pl-[0.1rem]";

const GROUP_TITLE = "text-[0.72rem] font-bold tracking-[0.05em] text-ink-muted uppercase";

const GROUP_HINT = "text-[0.76rem] text-ink-body";

const LIST = "flex list-none flex-col gap-2";

const LIST_EMPTY =
  "rounded border border-dashed border-line px-4 py-4 text-center text-[0.8rem] text-ink-muted";

const ROW_BASE =
  "flex items-start gap-[0.9rem] rounded-lg border border-line bg-card px-4 py-[0.85rem] shadow-card transition-[border-color,box-shadow] duration-150 hover:border-ink-subtle";

const ROW_ICON =
  "flex size-8 shrink-0 items-center justify-center rounded-md border border-line-subtle bg-page text-ink-body";

const ROW_ICON_PINNED = "border-primary-border bg-primary-subtle text-primary-ink";

const ROW_BODY = "flex min-w-0 flex-1 flex-col gap-[0.3rem]";

const ROW_TITLELINE = "flex flex-wrap items-center gap-2";

const ROW_TITLE = "text-[0.9rem] font-semibold tracking-[-0.005em] text-ink";

const ROW_DESC = "max-w-[78ch] text-[0.79rem] leading-[1.45] text-ink-body";

const ROW_META = "mt-[0.1rem] flex flex-wrap gap-[0.3rem]";

const ROW_NOTE = "text-[0.75rem] text-ink-muted italic";

const ROW_PROBLEM =
  "mt-[0.15rem] flex items-start gap-[0.35rem] text-[0.76rem] leading-[1.45] text-danger [&_svg]:mt-px [&_svg]:shrink-0";

const ROW_ACTIONS = "flex shrink-0 items-center gap-[0.6rem] self-center";

const ROW_OPEN =
  "inline-flex items-center gap-[0.25rem] rounded px-[0.35rem] py-1 text-[0.76rem] font-semibold text-primary-ink no-underline hover:underline";

/** The small uppercase chip reporting the server's load state. */
const STATE_CHIP = "uppercase";

const BADGE_LOCKED = "uppercase";

const TAG =
  "inline-flex items-center rounded-[3px] border border-line-subtle bg-page px-[0.4rem] py-[0.1rem] text-[0.68rem] font-medium text-ink-body";

const TAG_UPSTREAM = "border-primary-border bg-primary-subtle text-primary-ink";

/*
 * Written out as literal class strings rather than interpolated from a colour
 * variable: Tailwind scans source text for complete class names, so a
 * `text-[${tint}]` template compiles to nothing at all and the wash would
 * render unstyled. `color-mix` still derives it from the live token, so a theme
 * redefining danger or success is followed.
 */
const STATE_CHIP_ERROR =
  "border-[color-mix(in_srgb,var(--danger)_30%,var(--bg-card))] bg-[color-mix(in_srgb,var(--danger)_8%,var(--bg-card))] text-danger";

const FEEDBACK_ERROR =
  "mt-[0.2rem] inline-flex items-center gap-[0.4rem] rounded border border-[color-mix(in_srgb,var(--danger)_28%,var(--bg-card))] bg-[color-mix(in_srgb,var(--danger)_7%,var(--bg-card))] py-[0.2rem] pr-[0.3rem] pl-[0.5rem] text-[0.76rem] leading-[1.4] text-danger";

const FEEDBACK_SUCCESS =
  "mt-[0.2rem] inline-flex items-center gap-[0.4rem] rounded border border-[color-mix(in_srgb,var(--success)_28%,var(--bg-card))] bg-[color-mix(in_srgb,var(--success)_8%,var(--bg-card))] py-[0.2rem] pr-[0.3rem] pl-[0.5rem] text-[0.76rem] leading-[1.4] text-success";

function feedbackClass(kind: PluginFeedback["kind"]): string {
  return kind === "error" ? FEEDBACK_ERROR : FEEDBACK_SUCCESS;
}

/*
 * The two state chips are `Badge`s, not bespoke spans.
 *
 * The chip *is* a badge -- a short label whose meaning is its text and whose
 * colour is a status token -- and the ported `Badge` already carries the focus
 * ring, the disabled state and the `[&_svg]` sizing the hand-rolled version was
 * missing. `STATE_CHIP_ERROR` is layered on through `className`, so a failed load
 * is still announced in danger ink rather than the neutral chip colour.
 */
const CHIP_SIZE = "sm";

/*
 * The loading placeholder.
 *
 * The ported `Skeleton` supplies the shimmer and the rounding; only its
 * dimensions are ours, because a console row has a fixed height the shimmer has
 * to fill without the layout moving when the data lands.
 */
const SKELETON_LINE = "h-[9px] w-full max-w-[34rem]";

const SKELETON_TITLE = "h-[13px] w-64 max-w-full";

const SKELETON_TAIL = "h-[9px] w-80 max-w-full";

const SKELETON_SWITCH = "h-[21px] w-[74px] rounded-[11px]";

/* ---------------------------------------------------------------- switch */

/*
 * The ported Base UI `Switch`, rendered as a real `<button>`.
 *
 * The switch carries a text label ("On"/"Off"/"Turning on"), a busy state and a
 * locked state, and `pages.spec.ts` asserts the markup: `role="switch"`,
 * `aria-checked`, `aria-labelledby` pointing at the row heading, and the
 * `disabled` attribute for the always-on row.
 *
 * `nativeButton` + `render={<button />}` is what buys all four. Base UI's
 * default is a non-native control: a `<span role="switch">` that mirrors
 * `disabled` as `aria-disabled` and drops itself out of the tab order. That is
 * defensible in general, but it is the wrong trade for the locked row — the
 * reason the dashboard plugin cannot be turned off lives in
 * `aria-describedby`, and a control no keyboard user can reach is a reason
 * nobody hears. Asked for a native button, Base UI keeps `disabled=""` on the
 * element and lets the browser drop it from the tab order, which is the
 * behaviour the markup assertions describe.
 *
 * The label rides alongside in a sibling span rather than inside the control:
 * Base UI renders a thumb of its own, and a `<label>` wrapping both wires the
 * visible word to the switch without an `aria-label` fighting it.
 */
const SWITCH_LABEL = "cursor-pointer disabled:cursor-not-allowed";

const SWITCH_TEXT = "min-w-[3.1rem] text-left text-[0.74rem] font-semibold text-ink-body";

function PluginSwitch({
  plugin,
  busy,
  onToggle,
}: {
  plugin: PluginStatus;
  busy: boolean;
  onToggle: (plugin: PluginStatus, enabled: boolean) => void;
}) {
  const locked = isAlwaysOn(plugin);
  const next = !plugin.enabled;
  return (
    <label className={cn(SWITCH_LABEL, "flex items-center gap-[0.45rem]")}>
      <Switch
        nativeButton
        render={<button />}
        checked={plugin.enabled}
        disabled={locked || busy}
        onCheckedChange={(on: boolean) => onToggle(plugin, on)}
        aria-labelledby={titleId(plugin)}
        aria-describedby={locked ? `${titleId(plugin)}-note` : undefined}
        aria-busy={busy || undefined}
        className={cn("m-0 shrink-0", locked && "opacity-70")}
      />
      <span className={cn(SWITCH_TEXT, plugin.enabled && "text-ink")}>
        {busy ? (next ? "Turning on" : "Turning off") : plugin.enabled ? "On" : "Off"}
      </span>
    </label>
  );
}

/* ------------------------------------------------------------------ row */

function PluginRow({
  plugin,
  busy,
  feedback,
  onToggle,
  onDismiss,
}: {
  plugin: PluginStatus;
  busy: boolean;
  feedback: PluginFeedback | undefined;
  onToggle: (plugin: PluginStatus, enabled: boolean) => void;
  onDismiss: (id: string) => void;
}) {
  const skills = countSkills(plugin);
  const locked = isAlwaysOn(plugin);
  const failed = plugin.state === "error";

  return (
    <li
      className={cn(
        ROW_BASE,
        locked && "border-l-[3px] border-l-primary",
        failed && "border-l-[3px] border-l-danger",
      )}
    >
      <div className={cn(ROW_ICON, locked && ROW_ICON_PINNED)} aria-hidden="true">
        {locked ? <LayersIcon /> : <PlugIcon />}
      </div>

      <div className={ROW_BODY}>
        <div className={ROW_TITLELINE}>
          <h3 className={ROW_TITLE} id={titleId(plugin)}>
            {plugin.name}
          </h3>
          <Badge
            size={CHIP_SIZE}
            className={cn(STATE_CHIP, plugin.state === "error" && STATE_CHIP_ERROR)}
          >
            {describeState(plugin)}
          </Badge>
          {locked && (
            <Badge size={CHIP_SIZE} className={BADGE_LOCKED} title={ALWAYS_ON_REASON}>
              Always on
            </Badge>
          )}
        </div>

        <p className={ROW_DESC}>{plugin.description}</p>

        <div className={ROW_META}>
          {plugin.category && <span className={TAG}>{plugin.category}</span>}
          {plugin.upstream?.product && (
            <span className={cn(TAG, TAG_UPSTREAM)}>
              Upstream · {plugin.upstream.product}
              {plugin.upstream.envPrefix ? ` (${plugin.upstream.envPrefix})` : ""}
            </span>
          )}
          {plugin.agent && (
            <span className={TAG}>
              {skills} agent {skills === 1 ? "skill" : "skills"}
            </span>
          )}
          <span className={cn(TAG, "text-ink-muted")}>v{plugin.version}</span>
        </div>

        {locked && (
          <p className={ROW_NOTE} id={`${titleId(plugin)}-note`}>
            {ALWAYS_ON_REASON}
          </p>
        )}

        {failed && plugin.error && (
          <p className={ROW_PROBLEM} role="status">
            <AlertIcon />
            <span>{plugin.error}</span>
          </p>
        )}

        {feedback && (
          <p className={feedbackClass(feedback.kind)} role="status">
            <span>{feedback.message}</span>
            <button
              type="button"
              className="cursor-pointer rounded-[3px] border-none bg-none px-[0.25rem] text-[0.95rem] leading-none text-inherit opacity-75 hover:opacity-100"
              onClick={() => onDismiss(plugin.id)}
              aria-label={`Dismiss ${plugin.name} message`}
            >
              &times;
            </button>
          </p>
        )}
      </div>

      <div className={ROW_ACTIONS}>
        {plugin.enabled && (
          <Link to={pluginHref(plugin)} className={ROW_OPEN}>
            Open
            <ArrowRightIcon />
          </Link>
        )}
        <PluginSwitch plugin={plugin} busy={busy} onToggle={onToggle} />
      </div>
    </li>
  );
}

function ConsoleSkeleton() {
  return (
    <ul className={LIST} aria-hidden="true">
      {[0, 1, 2].map((index) => (
        <li key={index} className={cn(ROW_BASE, "items-center")}>
          <div className={ROW_ICON} />
          <div className={ROW_BODY}>
            <Skeleton className={SKELETON_TITLE} />
            <Skeleton className={SKELETON_LINE} />
            <Skeleton className={SKELETON_TAIL} />
          </div>
          <div className={ROW_ACTIONS}>
            <Skeleton className={SKELETON_SWITCH} />
          </div>
        </li>
      ))}
    </ul>
  );
}

/** The banner shown when the list itself could not be read. */
function ConsoleError({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <Alert variant="error">
      <AlertIcon />
      <AlertTitle>Could not load the plugin list.</AlertTitle>
      <AlertDescription>{message}</AlertDescription>
      <AlertAction>
        <button type="button" className={cn(BTN_BASE, BTN_OUTLINE, BTN_SM)} onClick={onRetry}>
          Retry
        </button>
      </AlertAction>
    </Alert>
  );
}

/* ----------------------------------------------------------------- page */

export function ConsolePage() {
  const { plugins, loading, error, refresh, setEnabled, pending, feedback, dismissFeedback } =
    usePlugins();
  const { pinned, rest } = partitionPlugins(plugins);
  const enabledCount = plugins.filter((plugin) => plugin.enabled).length;

  const handleToggle = (plugin: PluginStatus, enabled: boolean) => {
    void setEnabled(plugin, enabled);
  };
  const handleRefresh = () => void refresh().catch(() => undefined);

  const row = (plugin: PluginStatus) => (
    <PluginRow
      key={plugin.id}
      plugin={plugin}
      busy={Boolean(pending[plugin.id])}
      feedback={feedback[plugin.id]}
      onToggle={handleToggle}
      onDismiss={dismissFeedback}
    />
  );

  return (
    <div className={PAGE}>
      <header className={PAGE_HEAD}>
        <div className={PAGE_HEAD_TEXT}>
          <h1 className={PAGE_TITLE}>Plugin console</h1>
          <p className={PAGE_SUBTITLE}>
            Every plugin the host knows about. Turning one on loads it fully — its routes and its
            agent skills start answering. Turning one off disposes it.
          </p>
        </div>
        <div className={PAGE_HEAD_ACTIONS}>
          <p className={PAGE_COUNT}>
            <strong>{enabledCount}</strong> of <strong>{plugins.length}</strong> on
          </p>
          <button
            type="button"
            className={cn(BTN_BASE, BTN_OUTLINE)}
            onClick={handleRefresh}
            disabled={loading}
          >
            Refresh
          </button>
        </div>
      </header>

      {error && <ConsoleError message={error} onRetry={handleRefresh} />}

      {loading && plugins.length === 0 ? (
        <ConsoleSkeleton />
      ) : (
        <>
          <section className={GROUP} aria-labelledby="console-group-product">
            <div className={GROUP_HEAD}>
              <h2 className={GROUP_TITLE} id="console-group-product">
                Product
              </h2>
              <p className={GROUP_HINT}>{ALWAYS_ON_REASON}.</p>
            </div>
            <ul className={LIST}>
              {pinned.length === 0 ? (
                <li className={LIST_EMPTY}>No always-on plugin is registered.</li>
              ) : (
                pinned.map(row)
              )}
            </ul>
          </section>

          <section className={cn(GROUP, "mt-[0.35rem]")} aria-labelledby="console-group-available">
            <div className={GROUP_HEAD}>
              <h2 className={GROUP_TITLE} id="console-group-available">
                Available plugins
              </h2>
              <p className={GROUP_HINT}>
                Enabled plugins appear on the{" "}
                <Link to="/plugins" className="text-primary-ink underline">
                  plugins page
                </Link>
                .
              </p>
            </div>
            <ul className={LIST}>
              {rest.length === 0 ? (
                <li className={LIST_EMPTY}>No other plugins are installed.</li>
              ) : (
                rest.map(row)
              )}
            </ul>
          </section>
        </>
      )}
    </div>
  );
}
