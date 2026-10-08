// The desktop edition's entry (AP1e): the same console, with the Electron
// platform and the desktop catalog patch. main.tsx calls it when the preload
// exposed `window.loamsDesktop`.

import { type ConsoleHandle, parsePatch } from '@loams/console-host';
import {
  createElectronPlatform,
  type LoamsDesktopApi,
  wireDeepLinks,
} from '@loams/platform-electron';
import dataStudioPkg from '@loams/plugin-data-studio/package.json';
import desktopServersPkg from '@loams/plugin-desktop-servers/package.json';
import desktopYml from '../../catalog/desktop.yml?raw';
import type { modules } from './modules.js';
import { startConsole } from './start.js';

/** Desktop-only bundled plugins, by package name; each plugin task adds its own. */
export const desktopModules: typeof modules = {
  '@loams/plugin-data-studio': () => import('@loams/plugin-data-studio'),
  '@loams/plugin-desktop-servers': () => import('@loams/plugin-desktop-servers'),
};
/** Their package.json manifests. */
export const desktopManifests: unknown[] = [desktopServersPkg, dataStudioPkg];

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
  });
  await wireDeepLinks(api);
  return handle;
}
