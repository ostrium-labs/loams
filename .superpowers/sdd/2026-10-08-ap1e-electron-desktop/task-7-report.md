# Task 7 report: platform-electron, desktop catalog patch, console desktop entry

## Done
- `web/packages/platform-electron`: `createElectronPlatform(api)` provides `platform` (kind 'desktop', baseUrl = origin, IPC side effects; openExternal throws if the bridge refuses), `transport` (Connect, binary), `desktop` (the API).
  - C0.1: interceptor merges `features.desktop=true` and (active kind local) `features.local=true` into `GetInstance`.
  - Demo auth: fetch adds `Bearer mock-access-usr_omar` only when the active kind is 'demo' (kind read once, cached).
  - `wireDeepLinks`: subscribe `onNavigate` first, then `takePendingNavigation`; sets `location.hash`.
- Type sharing: `@loams/desktop` gets `exports: {"./contracts": ...}`; platform-electron has it as devDependency, `import type` only. `contracts.ts` used `NodeJS.Platform`, which broke the console typecheck (no @types/node there), so it is now a spelled-out `DesktopPlatform` union (same members).
- `catalog/desktop.yml`: comment plus `[]` (a comment-only file parses to null and `parsePatch` throws). `src/cordis/desktop.ts` has `desktopModules`/`desktopManifests` (empty; later plugin tasks add entries) and `startDesktop`. `main.tsx` calls it when `globalThis.loamsDesktop` exists; web path unchanged. CSP unchanged.
- Classic `shell.tsx`: a "Desktop" link to `/ui/cordis.html` when `window.loamsDesktop` exists. Global typed in `src/desktop-env.d.ts`.
- `PlatformService.kind` already included 'desktop'. modules.ts unchanged (no desktop plugins yet).

## Tests
- platform-electron: 11 pass (the named tests plus bearer per kind, features per kind, deep-link order, openExternal refusal, desktop plugin loads under platform-electron).
- console: 34 pass (new `desktop_catalog_parses`); desktop-electron: 116 pass. tsc clean (platform-electron, console, desktop). `@loams/console` build ok. `tools/monorepo/check.py` passes. Biome clean.

## Manual run
Built Electron app, fresh user-data-dir, servers.json activeId 'demo', loams-apps-mock on :8084. The cordis console booted: shell, rpc, identity, stack-status, namespaces, approvals active, hello skipped, nothing pending. WhoAmI succeeded through the proxy ("2 approvals waiting"), badge "Loams Desktop". Screenshot via CDP Page.captureScreenshot: task-7-shot.png.

## Concerns
- Only the classic-to-Desktop link was added (per brief); no link from cordis back to the classic console (R0.4 suggested a slot entry).
- Platform kind 'desktop' confirmed indirectly (bearer applied, Desktop badge); the handle does not expose the platform.
