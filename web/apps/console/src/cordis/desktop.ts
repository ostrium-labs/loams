// The desktop edition's entry (AP1e): the same console, with the Electron
// platform and the desktop catalog patch. main.tsx calls it when the preload
// exposed `window.loamsDesktop`.

import { type ConsoleHandle, type ModeService, parsePatch } from '@loams/console-host';
import {
  createElectronPlatform,
  type LoamsDesktopApi,
  wireDeepLinks,
  wireShellCommands,
} from '@loams/platform-electron';
import agentPkg from '@loams/plugin-agent/package.json';
import canvasPkg from '@loams/plugin-canvas/package.json';
import connectorsPkg from '@loams/plugin-connectors/package.json';
import dataStudioPkg from '@loams/plugin-data-studio/package.json';
import desktopAgentdPkg from '@loams/plugin-desktop-agentd/package.json';
import desktopServersPkg from '@loams/plugin-desktop-servers/package.json';
import desktopSettingsPkg from '@loams/plugin-desktop-settings/package.json';
import durablePkg from '@loams/plugin-durable/package.json';
import enginesPkg from '@loams/plugin-engines/package.json';
import factoryPkg from '@loams/plugin-factory/package.json';
import gitPkg from '@loams/plugin-git/package.json';
import graphPkg from '@loams/plugin-graph/package.json';
import housePkg from '@loams/plugin-house/package.json';
import jobsPkg from '@loams/plugin-jobs/package.json';
import livePkg from '@loams/plugin-live/package.json';
import metricsPkg from '@loams/plugin-metrics/package.json';
import overviewPkg from '@loams/plugin-overview/package.json';
import pipelinesPkg from '@loams/plugin-pipelines/package.json';
import postgresPkg from '@loams/plugin-postgres/package.json';
import sqlPkg from '@loams/plugin-sql/package.json';
import streamsPkg from '@loams/plugin-streams/package.json';
import desktopYml from '../../catalog/desktop.yml?raw';
import type { modules } from './modules.js';
import { startConsole } from './start.js';

/** Desktop-only bundled plugins, by package name; each plugin task adds its own. */
export const desktopModules: typeof modules = {
  '@loams/plugin-data-studio': () => import('@loams/plugin-data-studio'),
  '@loams/plugin-desktop-servers': () => import('@loams/plugin-desktop-servers'),
  '@loams/plugin-factory': () => import('@loams/plugin-factory'),
  '@loams/plugin-durable': () => import('@loams/plugin-durable'),
  '@loams/plugin-connectors': () => import('@loams/plugin-connectors'),
  '@loams/plugin-graph': () => import('@loams/plugin-graph'),
  '@loams/plugin-jobs': () => import('@loams/plugin-jobs'),
  '@loams/plugin-house': () => import('@loams/plugin-house'),
  '@loams/plugin-git': () => import('@loams/plugin-git'),
  '@loams/plugin-pipelines': () => import('@loams/plugin-pipelines'),
  '@loams/plugin-streams': () => import('@loams/plugin-streams'),
  '@loams/plugin-overview': () => import('@loams/plugin-overview'),
  '@loams/plugin-desktop-settings': () => import('@loams/plugin-desktop-settings'),
  '@loams/plugin-live': () => import('@loams/plugin-live'),
  '@loams/plugin-agent': () => import('@loams/plugin-agent'),
  '@loams/plugin-postgres': () => import('@loams/plugin-postgres'),
  '@loams/plugin-sql': () => import('@loams/plugin-sql'),
  '@loams/plugin-desktop-agentd': () => import('@loams/plugin-desktop-agentd'),
  '@loams/plugin-canvas': () => import('@loams/plugin-canvas'),
  '@loams/plugin-metrics': () => import('@loams/plugin-metrics'),
  '@loams/plugin-engines': () => import('@loams/plugin-engines'),
};
/** Their package.json manifests. */
export const desktopManifests: unknown[] = [
  desktopServersPkg,
  dataStudioPkg,
  factoryPkg,
  durablePkg,
  connectorsPkg,
  graphPkg,
  jobsPkg,
  housePkg,
  gitPkg,
  pipelinesPkg,
  streamsPkg,
  overviewPkg,
  desktopSettingsPkg,
  livePkg,
  agentPkg,
  postgresPkg,
  sqlPkg,
  desktopAgentdPkg,
  canvasPkg,
  metricsPkg,
  enginesPkg,
];

export async function startDesktop(
  api: LoamsDesktopApi,
  root: HTMLElement,
): Promise<ConsoleHandle> {
  const handle = await startConsole({
    platform: createElectronPlatform(api),
    root,
    patches: [parsePatch(desktopYml)],
    extraModules: desktopModules,
    extraManifests: desktopManifests,
    // Studio is the desktop's default mode and needs no instance flag (D902, Q801).
    modeFlags: {},
  });
  // The application menu's Studio, Console, Switch mode and New thread (SD1 Task 28).
  const mode = handle.ctx.get('mode') as ModeService | undefined;
  if (mode) wireShellCommands(api, mode);
  await wireDeepLinks(api);
  // The splash closes once the console has painted (D1084).
  requestAnimationFrame(() => void api.shell.ready?.());
  return handle;
}
