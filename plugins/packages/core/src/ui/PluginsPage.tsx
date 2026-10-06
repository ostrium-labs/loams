/**
 * The plugins page: what is switched on, and one click into each.
 *
 * Deliberately only ever lists *enabled* plugins -- that is the contract the
 * console toggle exists to satisfy, so this page never needs a second source of
 * truth about what is available.
 */
import React from "react";
import { Link } from "react-router";
import type { PluginStatus } from "../types.js";
import {
  countSkills,
  describeState,
  enabledPlugins,
  isAlwaysOn,
  partitionPlugins,
  pluginHref,
} from "./pluginState.js";
import { usePlugins } from "./usePlugins.jsx";
import { cn } from "./cn.js";
import {
  BTN_BASE,
  BTN_OUTLINE,
  BTN_PRIMARY,
  BTN_SM,
  EMPTY_STATE,
  EMPTY_STATE_BODY,
  EMPTY_STATE_MARK,
  EMPTY_STATE_TITLE,
  PAGE,
  PAGE_HEAD,
  PAGE_HEAD_ACTIONS,
  PAGE_HEAD_TEXT,
  PAGE_SUBTITLE,
  PAGE_TITLE,
  SKELETON_LINE,
  SKELETON_TITLE,
} from "./uiClasses.js";
import { AlertIcon, ArrowRightIcon, GaugeIcon, PlugIcon } from "./icons.js";
import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
  Badge,
  Skeleton,
} from "./primitives/index.js";

/* ------------------------------------------------------------- fragments */

const GRID = "grid list-none grid-cols-[repeat(auto-fill,minmax(288px,1fr))] items-start gap-3";

const CARD =
  "flex h-full items-start gap-3 rounded-lg border border-line bg-card px-4 py-[0.95rem] text-inherit no-underline shadow-card transition-[border-color,box-shadow,transform] duration-150 hover:-translate-y-px hover:border-primary-border hover:shadow-hover";

const CARD_SKELETON = "pointer-events-none";

const CARD_ICON =
  "flex size-8 shrink-0 items-center justify-center rounded-md bg-primary-subtle text-primary-ink";

const CARD_MAIN = "flex min-w-0 flex-1 flex-col gap-[0.3rem]";

const CARD_TITLELINE = "flex flex-wrap items-center gap-[0.45rem]";

const CARD_TITLE = "text-[0.88rem] font-semibold tracking-[-0.005em] text-ink";

const CARD_DESC = "line-clamp-3 overflow-hidden text-[0.77rem] leading-[1.45] text-ink-body";

const CARD_META = "mt-[0.15rem] flex flex-wrap gap-[0.3rem]";

const CARD_GO =
  "inline-flex shrink-0 self-center text-ink-muted transition-transform duration-150 group-hover:translate-x-[2px] group-hover:text-primary";

const BADGE_LOCKED = "uppercase";

const STATE_CHIP = "uppercase";

const TAG =
  "inline-flex items-center rounded-[3px] border border-line-subtle bg-page px-[0.4rem] py-[0.1rem] text-[0.68rem] font-medium text-ink-body";

const TAG_UPSTREAM = "border-primary-border bg-primary-subtle text-primary-ink";

/*
 * Written out rather than interpolated from a colour variable: Tailwind scans
 * source text for complete class names, so a `text-[${tint}]` template compiles
 * to nothing at all and the chip would render unstyled. `color-mix` still
 * derives it from the live token, so a theme redefining danger is followed.
 */
const STATE_CHIP_ERROR =
  "border-[color-mix(in_srgb,var(--danger)_30%,var(--bg-card))] bg-[color-mix(in_srgb,var(--danger)_8%,var(--bg-card))] text-danger";

function PluginCard({ plugin }: { plugin: PluginStatus }) {
  const skills = countSkills(plugin);
  const pinned = isAlwaysOn(plugin);

  return (
    <li>
      {/*
       * An anchor, not a div with onClick: it is focusable, activates on Enter,
       * opens in a new tab on cmd-click, and is announced as a link. React
       * Router's `Link` intercepts only the plain-left-click case and leaves
       * the browser's default for everything else.
       */}
      <Link to={pluginHref(plugin)} className={cn(CARD, "group")}>
        <span className={CARD_ICON} aria-hidden="true">
          {pinned ? <GaugeIcon /> : <PlugIcon />}
        </span>

        <span className={CARD_MAIN}>
          <span className={CARD_TITLELINE}>
            <span className={CARD_TITLE}>{plugin.name}</span>
            {pinned && (
              <Badge size="sm" className={BADGE_LOCKED}>
                Always on
              </Badge>
            )}
          </span>
          <span className={CARD_DESC}>{plugin.description}</span>
          <span className={CARD_META}>
            {plugin.category && <span className={TAG}>{plugin.category}</span>}
            {plugin.upstream?.product && (
              <span className={cn(TAG, TAG_UPSTREAM)}>Upstream · {plugin.upstream.product}</span>
            )}
            {plugin.agent && (
              <span className={TAG}>
                {skills} agent {skills === 1 ? "skill" : "skills"}
              </span>
            )}
            <Badge
              size="sm"
              className={cn(STATE_CHIP, plugin.state === "error" && STATE_CHIP_ERROR)}
            >
              {describeState(plugin)}
            </Badge>
          </span>
        </span>

        <span className={CARD_GO} aria-hidden="true">
          <ArrowRightIcon />
        </span>
      </Link>
    </li>
  );
}

/** The banner shown when the list itself could not be read. */
function PluginsError({ message, onRetry }: { message: string; onRetry: () => void }) {
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

export function PluginsPage() {
  const { plugins, loading, error, refresh } = usePlugins();
  const { pinned, rest } = partitionPlugins(enabledPlugins(plugins));
  const nothingOn = !loading && error === null && pinned.length === 0 && rest.length === 0;
  const handleRefresh = () => void refresh().catch(() => undefined);

  return (
    <div className={PAGE}>
      <header className={PAGE_HEAD}>
        <div className={PAGE_HEAD_TEXT}>
          <h1 className={PAGE_TITLE}>Plugins</h1>
          <p className={PAGE_SUBTITLE}>
            The plugins that are switched on right now. Turn more on from the plugin toggle panel.
          </p>
        </div>
        <div className={PAGE_HEAD_ACTIONS}>
          <button
            type="button"
            className={cn(BTN_BASE, BTN_OUTLINE)}
            onClick={handleRefresh}
            disabled={loading}
          >
            Refresh
          </button>
          <Link to="/console" className={cn(BTN_BASE, BTN_PRIMARY)}>
            Open console
          </Link>
        </div>
      </header>

      {error && <PluginsError message={error} onRetry={handleRefresh} />}

      {loading && plugins.length === 0 ? (
        <ul className={GRID} aria-hidden="true">
          {[0, 1, 2].map((index) => (
            <li key={index}>
              <div className={cn(CARD, CARD_SKELETON)}>
                <div className={CARD_ICON} />
                <div className={CARD_MAIN}>
                  <Skeleton className={SKELETON_TITLE} />
                  <Skeleton className={SKELETON_LINE} />
                </div>
              </div>
            </li>
          ))}
        </ul>
      ) : nothingOn ? (
        <div className={EMPTY_STATE}>
          <div className={EMPTY_STATE_MARK} aria-hidden="true">
            <PlugIcon />
          </div>
          <h2 className={EMPTY_STATE_TITLE}>No plugins are switched on</h2>
          <p className={EMPTY_STATE_BODY}>
            Everything the host can load is currently off. The console lists every available plugin
            with a switch — turn one on and it will show up here.
          </p>
          <Link to="/console" className={cn(BTN_BASE, BTN_PRIMARY)}>
            Open the plugin console
          </Link>
        </div>
      ) : (
        <ul className={GRID}>
          {pinned.map((plugin) => (
            <PluginCard key={plugin.id} plugin={plugin} />
          ))}
          {rest.map((plugin) => (
            <PluginCard key={plugin.id} plugin={plugin} />
          ))}
        </ul>
      )}
    </div>
  );
}
