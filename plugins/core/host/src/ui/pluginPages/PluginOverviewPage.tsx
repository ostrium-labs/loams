/**
 * The default page for a plugin that has no bespoke UI yet.
 *
 * This is what makes an upstream plugin useful on day one: the page is honest
 * about not having a real surface, and it still shows everything the manifest
 * does carry -- what the plugin is, what it talks to, and the skills its agent
 * advertises.
 */
import React from "react";
import { Link } from "react-router";
import type { PluginAgentSkill, PluginStatus } from "../../types.js";
import { describeState } from "../pluginState.js";
import { cn } from "../cn.js";
import {
  BTN_BASE,
  BTN_OUTLINE,
  BTN_SM,
  PAGE,
  PAGE_HEAD,
  PAGE_HEAD_ACTIONS,
  PAGE_HEAD_TEXT,
  PAGE_SUBTITLE,
  PAGE_TITLE,
} from "../uiClasses.js";
import { AlertIcon } from "../icons.js";

/* ------------------------------------------------------------- fragments */

const TITLINE = "flex flex-wrap items-center gap-[0.55rem]";

const STATE_CHIP =
  "rounded-[3px] border border-line bg-page px-[0.45rem] py-[0.12rem] text-[0.65rem] font-bold tracking-[0.04em] whitespace-nowrap text-ink-body uppercase";

const TAG =
  "inline-flex items-center rounded-[3px] border border-line-subtle bg-page px-[0.4rem] py-[0.1rem] text-[0.68rem] font-medium text-ink-body";

const TAG_UPSTREAM = "border-primary-border bg-primary-subtle text-primary-ink";

const META = "mt-[0.1rem] flex flex-wrap gap-[0.3rem]";

const NOTICE =
  "rounded border border-primary-border border-l-[3px] border-l-primary bg-primary-subtle px-4 py-3";

const NOTICE_TITLE = "text-[0.85rem] font-semibold text-primary-ink";

const NOTICE_BODY = "mt-1 text-[0.79rem] leading-[1.5] text-ink-body";

const SECTION = "flex flex-col gap-3";

const SECTION_HEAD = "flex flex-wrap items-baseline gap-3";

const SECTION_TITLE = "text-[0.72rem] font-bold tracking-[0.05em] text-ink-muted uppercase";

const SECTION_HINT = "text-[0.76rem] text-ink-body";

const SECTION_EMPTY = "text-[0.8rem] text-ink-muted";

const SKILL_GRID =
  "grid list-none grid-cols-[repeat(auto-fill,minmax(260px,1fr))] items-start gap-3";

const SKILL_CARD =
  "flex flex-col gap-2 rounded-lg border border-line bg-card px-4 py-3 shadow-card";

const SKILL_CARD_HEAD = "flex items-center justify-between gap-2";

const SKILL_CARD_NAME = "text-[0.85rem] font-semibold text-ink";

const SKILL_CARD_ID =
  "rounded-[3px] border border-line-subtle bg-page px-1.5 py-0.5 font-mono text-[0.68rem] text-ink-muted";

const SKILL_CARD_DESC = "text-[0.78rem] leading-[1.45] text-ink-body";

function stateChip(plugin: PluginStatus): string {
  return cn(
    STATE_CHIP,
    plugin.state === "error" &&
      "border-[color-mix(in_srgb,var(--danger)_30%,var(--bg-card))] bg-[color-mix(in_srgb,var(--danger)_8%,var(--bg-card))] text-danger",
  );
}

function SkillCard({ skill }: { skill: PluginAgentSkill }) {
  return (
    <li className={SKILL_CARD}>
      <div className={SKILL_CARD_HEAD}>
        <h3 className={SKILL_CARD_NAME}>{skill.name}</h3>
        <code className={SKILL_CARD_ID}>{skill.id}</code>
      </div>
      <p className={SKILL_CARD_DESC}>{skill.description}</p>
      {skill.tags && skill.tags.length > 0 && (
        <div className="flex flex-wrap gap-[0.3rem]">
          {skill.tags.map((tag) => (
            <span className={TAG} key={tag}>
              {tag}
            </span>
          ))}
        </div>
      )}
    </li>
  );
}

export function PluginOverviewPage({ plugin }: { plugin: PluginStatus }) {
  const skills = plugin.agent?.skills ?? [];

  return (
    <div className={PAGE}>
      <header className={PAGE_HEAD}>
        <div className={PAGE_HEAD_TEXT}>
          <div className={TITLINE}>
            <h1 className={PAGE_TITLE}>{plugin.name}</h1>
            <span className={stateChip(plugin)}>{describeState(plugin)}</span>
          </div>
          <p className={PAGE_SUBTITLE}>{plugin.description}</p>
          <div className={META}>
            {plugin.category && <span className={TAG}>{plugin.category}</span>}
            {plugin.upstream?.product && (
              <span className={cn(TAG, TAG_UPSTREAM)}>
                Upstream · {plugin.upstream.product}
                {plugin.upstream.envPrefix ? ` (${plugin.upstream.envPrefix})` : ""}
              </span>
            )}
            <span className={cn(TAG, "text-ink-muted")}>v{plugin.version}</span>
          </div>
        </div>
        <div className={PAGE_HEAD_ACTIONS}>
          <Link to="/plugins" className={cn(BTN_BASE, BTN_OUTLINE)}>
            All plugins
          </Link>
          <Link to="/console" className={cn(BTN_BASE, BTN_OUTLINE)}>
            Console
          </Link>
        </div>
      </header>

      {plugin.state === "error" && plugin.error && (
        <div
          className="flex items-center gap-3 rounded border border-[color-mix(in_srgb,var(--danger)_32%,var(--bg-card))] border-l-[3px] border-l-danger bg-[color-mix(in_srgb,var(--danger)_6%,var(--bg-card))] px-4 py-[0.8rem] text-danger [&_svg]:shrink-0"
          role="alert"
        >
          <AlertIcon />
          <div className="flex min-w-0 flex-1 flex-col gap-[0.15rem] text-[0.78rem]">
            <strong>This plugin failed to load.</strong>
            <span className="text-ink-body">{plugin.error}</span>
          </div>
          <Link to="/console" className={cn(BTN_BASE, BTN_OUTLINE, BTN_SM)}>
            Turn it off and on
          </Link>
        </div>
      )}

      <section className={NOTICE} aria-labelledby="plugin-notice-title">
        <h2 className={NOTICE_TITLE} id="plugin-notice-title">
          No dedicated interface for this plugin yet
        </h2>
        <p className={NOTICE_BODY}>
          The plugin is loaded and its routes and agent skills are answering, but it has not shipped
          a screen of its own yet. What the manifest declares is below.
        </p>
      </section>

      <section className={SECTION} aria-labelledby="plugin-skills-title">
        <div className={SECTION_HEAD}>
          <h2 className={SECTION_TITLE} id="plugin-skills-title">
            Agent skills
          </h2>
          <p className={SECTION_HINT}>
            {skills.length === 0
              ? "This plugin exposes no agent skills."
              : `${skills.length} skill${skills.length === 1 ? "" : "s"} this plugin's agent advertises.`}
          </p>
        </div>
        {skills.length === 0 ? (
          <p className={SECTION_EMPTY}>Nothing to show.</p>
        ) : (
          <ul className={SKILL_GRID}>
            {skills.map((skill) => (
              <SkillCard key={skill.id} skill={skill} />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
