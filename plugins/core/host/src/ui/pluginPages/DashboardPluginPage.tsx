/**
 * The dashboard, exposed as a plugin page.
 *
 * It is a thin adapter: the real dashboard component is injected by the host
 * through `DashboardSlotContext`, because that component belongs to
 * `@loams-core/dashboard-ui` and core is below it in the dependency graph.
 */
import React from "react";
import type { PluginStatus } from "../../types.js";
import { useDashboardSlot } from "../DashboardSlot.js";
import { Link } from "react-router";
import { cn } from "../cn.js";
import {
  BTN_BASE,
  BTN_OUTLINE,
  EMPTY_STATE,
  EMPTY_STATE_BODY,
  EMPTY_STATE_MARK,
  EMPTY_STATE_TITLE,
  PAGE,
} from "../uiClasses.js";

export function DashboardMissingNotice({ name = "The dashboard" }: { name?: string }) {
  return (
    <div className={PAGE}>
      <div className={EMPTY_STATE}>
        <div className={EMPTY_STATE_MARK} aria-hidden="true">
          <svg
            width="22"
            height="22"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="m12 14 4-4" />
            <path d="M3.34 19a10 10 0 1 1 17.32 0" />
          </svg>
        </div>
        <h1 className={EMPTY_STATE_TITLE}>The dashboard was not mounted</h1>
        <p className={EMPTY_STATE_BODY}>
          <code>{name}</code> is registered as an always-on plugin, but the shell was not given a
          dashboard component. Render it with <code>&lt;CoreShell dashboard={"{App}"} /&gt;</code>{" "}
          in the host app.
        </p>
        <Link to="/console" className={cn(BTN_BASE, BTN_OUTLINE)}>
          Open the console
        </Link>
      </div>
    </div>
  );
}

/** The dashboard component the host mounted, or the missing-component notice. */
export function DashboardEntry() {
  const Dashboard = useDashboardSlot();
  if (!Dashboard) return <DashboardMissingNotice />;
  return <Dashboard />;
}

export function DashboardPluginPage({ plugin }: { plugin: PluginStatus }) {
  const Dashboard = useDashboardSlot();
  if (!Dashboard) return <DashboardMissingNotice name={plugin.name} />;
  return <Dashboard />;
}
