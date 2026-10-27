# CF1 — Console Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Loams console and Loams Desktop design 59's foundation, with no feature flag and no route lost: new token values and themes, the rest of T3 Code's primitives in `@loams/ui`, a six-region agent-first shell, and native desktop chrome, menus, splash and notifications.

**Architecture:** The tokens keep their `--op-*` names in `web/packages/ui/src/tokens.css` and take values derived from T3 Code (dark is direction B, light is direction A), so every page restyles through them. `@loams/ui` gains the rest of T3 Code's `components/ui` on Base UI, each file with a provenance row, and every existing export keeps its API. `@loams/plugin-shell` lays out six regions as `@loams/slots` slots: sidebar, top bar, main, inspector, drawer and palette. It adds the `inspector`, `drawer` and `theme` services and one keymap. Studio moves its sidebar, panel and terminal into those regions. Electron main gets per-OS window chrome synced to the page theme, native menus, a splash and the update toast, adapted from T3 Code and dsh-desktop (both MIT).

**Tech Stack:**
- Language and UI: TypeScript 5.9.3, React 19 (catalog), Base UI (`@base-ui/react`, catalog), Tailwind v4 (utilities only, `@theme inline` over the tokens), lucide-react, cordis plugins and `@loams/slots`.
- Tests: Vitest 5.0.3 with Testing Library 16.3.3 and jsdom 30.1.1; Playwright 1.63.0 (`@playwright/test`, catalog) with `@axe-core/playwright` 4.10.2.
- Desktop: Electron 44.4.5, electron-vite 5.0.0, electron-builder 26.16.1, and `gifenc` 1.0.3 for the splash loaders.

**Spec:** docs/design/59-console-foundation.md

## Global Constraints

- `--op-*` in `web/packages/ui/src/tokens.css` is the only source of colour, radius, shadow and spacing. A raw colour literal anywhere else under `web/plugins`, `web/packages` or `web/apps/console/src` fails `web/packages/ui/test/colours.test.ts` (D1073).
- Base UI only through `@loams/ui`: nothing outside `web/packages/ui` imports `@base-ui/react` (`web/packages/ui/test/boundaries.test.ts`, D1077).
- Every existing `@loams/ui` export keeps its name and props. Plugins do not change in this part, except a page that looks wrong after the token change, which is fixed in the task that broke it (D1073, D1086).
- New third-party dependencies follow CONTRIBUTING rule 3 and `deny.toml`, pass `scripts/ci/npm-licences.mjs`, and are pinned to a version published at least 14 days before it is added (§10).
- Every commit is signed off with `git commit -s`.
- Every file copied or adapted from T3 Code carries the §53 §9.2 header and a row in `web/packages/ui/provenance.json`. The row has the upstream path, commit `33aa4614783e856977a5f0b856dc65ebc1ce6794`, both SHA-256 sums and a verdict (`as-is`, `adapt` or `rewrite`).
- Every file adapted from dsh-desktop starts with `// Adapted from dataelement/dsh-desktop (MIT), <path>.` and has a row in `apps/desktop-electron/provenance.json` at commit `bc732f5`. `LICENSE.dsh-desktop` and NOTICE carry the attribution (§10).
- Never read, copy, install or bundle any `@deepseek-ai/*` package. The `dataelement/dsh-desktop` repository's own source is the only dsh source (§10).
- Every token pair used for text meets WCAG AA: 4.5:1, or 3:1 for large text, focus rings and UI boundaries. `web/packages/ui/test/contrast.test.ts` checks it (§3, §7).
- Monospace is for code, ids and SQL only, never for labels (D1075). `web/packages/ui/test/mono.test.ts` enforces it.
- The theme follows `prefers-color-scheme`, and is dark when the OS states no preference. The choice (System, Dark or Light) is per device: the web console keeps it in local storage, the desktop in its settings. `.dark` on `<html>` stays the switch (D1074).
- Motion is off by default: panels snap open. Colour and opacity transitions take 100–150ms. The optional panel easing runs up to 400ms on `cubic-bezier(0.32, 0.72, 0, 1)`. `prefers-reduced-motion: reduce` always wins (D1076).
- Every plugin page keeps its route, so links, bookmarks and the hash router keep working (D1080).
- No feature flag: everything lands on `dev`. Each task is its own PR into `dev` that leaves the console and the desktop working (D1086).
- Focus rings are the accent at 2px, offset 2px (§7).
- This plan touches no Rust. No `cargo` build is needed.

## Review Focus

These are the five unexercised inputs most likely to bite a person. Each one has a test in the task that owns the code.

1. **Shortcuts typed into editable targets.** ⌘B and ⌘I in Studio's rich-text composer must stay bold and italic. Ctrl+K, J, B, N and I inside a terminal on Linux and Windows must reach the shell program, not the console. Esc inside an input must not close the inspector. A person expects their text editor and terminal to keep their keys. *Test: Task 12, `keymap.test.ts` "editable targets keep their keys".*
2. **Many threads and very long names.** Picture 400 threads across 3 projects, titles of 300 characters, and project names with no spaces. Rows stay 28px and one line, truncate with a tooltip, and never scroll the sidebar sideways. Each project shows 10 threads until "Show more" is pressed. *Test: Task 9, `shell-sidebar.test.tsx` "long names truncate and big projects collapse".*
3. **Native chrome that does not follow a theme change.** The Windows and Linux `titleBarOverlay` colour, and the macOS backdrop, must update when the person switches theme. They must also update when the OS flips light or dark while the choice is System. *Test: Task 13, `window-chrome.test.ts` "chrome follows every theme change".*
4. **Pages that assume the old frame.** Some pages size themselves against the old 56px header (`100svh - var(--top,56px)`) or the full viewport (`100vh - 190px`). With a 44px top bar and a bottom drawer, they overflow or leave gaps. Pages must fill the main region. *Test: Task 7, `viewport.test.ts` "no page sizes itself against the viewport".*
5. **Forced colours (Windows High Contrast).** The new look relies on alpha hairlines, filled status dots and soft fills, which vanish under `forced-colors: active`. Borders, focus rings and status must stay visible, and status must not rely on colour alone. *Test: Task 1, `contrast.test.ts` "forced colours keep borders, focus and status".*

---

## Rulings made while writing this plan

The spec and the code disagree, or the spec is silent, in these places. Each ruling is the default the tasks implement. The report to the owner lists them.

| # | Finding | Ruling |
|---|---|---|
| W1 | Light muted text `#8a857c` (D1073) is 3.67:1 on white and 3.28:1 on the window. That fails the AA rule the same section requires. | `--op-muted` is `#6b665e` (5.7 on white, 4.96 on raised) and `--op-faint` is `#716c64` (≥ 4.53 on every light surface). `#8a857c` is not used for text. |
| W2 | The dark accent `oklch(0.571 0.21 264)` is 4.26:1 as text on the canvas, which fails AA. White on it is 4.64:1, which passes. | `--op-accent` keeps the spec value for fills, selection and focus (3:1 for UI is met). A new `--op-accent-fg` (dark `oklch(0.72 0.14 264)`, light = the accent) is the accent as text. `text-accent` is banned by `colours.test.ts`, and its 33 uses (T0-2) move to `text-accent-fg` in Task 1. |
| W3 | Ochre carried the meaning "warning" in `.loams-notice-warn`. Amber is a state colour (D1073), and there is no warn token. | New `--op-warn` and `--op-warn-soft` tokens (dark `#fbbf24`, light `#92400e`). |
| W4 | The light theme gives borders but not input outlines. Dark inputs are white at 8%. | New `--op-input` (dark `rgb(255 255 255 / 0.08)`, light `#dcd7cf`). |
| W5 | Today `rounded-none` maps to `--op-radius` (0). D1073 moves `--op-radius` to 10px. | Tailwind `rounded-none` becomes a literal `0`. The new steps are `rounded-xs`/`sm`/`md`/`lg`/`xl`/`pill`, and `rounded-panel` is `--op-radius`. The nine call sites that meant "the panel corner" move to a step in Task 1. |
| W6 | D1079 says thread status comes from "`@loams/studio-ui` `chat/store`". That file is the transcript store. The thread list (status, pin, reorder, archive) is `@loams/studio-ui` `thread-list/*` plus `plugins/studio/src/sidebar/*` over `ThreadService.WatchThreads`/`WatchSessions`. | The sidebar's thread section is Studio's existing `StudioSidebar`, mounted in the shell's `shell.sidebar.threads` slot. `chat/store` is not involved. |
| W7 | D1079 says "Projects come from the console's existing projects API". Threads belong to Studio's projects (agentd repos, `ProjectService.ListRepos`), not to the admin projects (`admin.ProjectService.ListProjects`). | The sidebar's projects → threads tree uses agentd repos. The admin projects feed the top bar's project pill and key the pinned Pages (D1080). The project pill's switcher replaces `shell.nav.context`. |
| W8 | D1078 names a `shell.main` slot. Main is already the router outlet (`console.page`, keyed by page id). | `shell.main` is the region name, not a new slot. The region renders `shell.home` (new, single) on the home route and `console.page` everywhere else. |
| W9 | The palette slot is named `shell.palette.source` (D1078), but `palette.command` already does this job (Studio uses it). | `shell.palette.source` is added with the same props. The shell reads both. `palette.command` stays as a documented alias, because changing a slot is a breaking change of the host. |
| W10 | Console and Studio modes (D900–D902) are not mentioned by design 59. The agent dock (`shell.dock.right`, ⌘J) collides with D1081's ⌘J (drawer). | Modes stay, and the mode switcher stays in the top bar's right cluster. Every available mode's plugins load at boot (`ModeController.prepare`, Task 9), so Studio's threads show in the sidebar in Console mode too, wherever an agentd is reachable (Q934); on the web the `studio` gate still decides whether Studio is available at all. The agent dock becomes the inspector's "Agent" tab. ⌘J is the drawer. Retiring the Console/Studio split is a question for Part 2 (recorded as an open question). |
| W11 | The home (§2) is "the existing Studio composer centred". Routes are kept, `/` is the Overview page, and Studio's home is `/studio`. | `shell.home` renders on `MODE_HOME[mode]` when that route has a home entry. In Studio mode, the desktop default, that is `/studio`, where the composer is centred. Console mode's `/` stays Overview until Part 2 decides. |
| W12 | D1081's ⌘⇧T "reopen a closed thread": Studio has no closed-thread list. | The shell keeps a per-session stack of thread routes left by navigation. ⌘⇧T reopens the most recent one. |
| W13 | The palette should search "thread text" (D1078). agentd has no transcript search; it has `FileService.SearchContent` for files only. | The palette matches thread titles and the last message preview, which Studio's palette source already carries. Full transcript search waits for an agentd method (open question). |
| W14 | D1084 says the splash uses dsh-desktop's loader generator. That generator needs `sharp`, whose prebuilt libvips binaries are LGPL-3.0, and the npm licence gate refused LGPL in SD1 (heic-to). | The adapted generator draws Loams' mark from its rectangles (`logo.tsx`), with no SVG rasteriser, and encodes with `gifenc` 1.0.3 (MIT). The GIFs are committed. The script runs on demand only. |
| W15 | D1085 "OS notifications for approvals and finished runs" already exist on the desktop (`index.ts` `studioWatch.onNotice`, `shouldNotify`). They check focus and open the thread on click. | Task 16 keeps them. It adds the dsh-style `window.Notification` shim on the existing `shell.notify` bridge for page code, and the web edition's equivalent through `platform.notify` (with click → route). "A long run" means any finished turn, error or run group while the window is unfocused. |
| W16 | D1086 lists four PR steps. The repository runs one PR per task. | The four steps are milestones CF1a–CF1d (below). Each task inside a milestone is its own PR, and the console works after each one. |
| W17 | §7 says screenshots run in `?demo` mode. `?demo` has no agentd, so it has no threads, inspector tabs or drawer tabs. | Screens that need threads run in the dev desktop preview (`/?desktop`, which carries `fake-agentd.ts`). The classic page and the web edition run in `/?demo`. Baselines live in `web/apps/console/test/screens/` (Q931's default). |
| W18 | The existing `web/packages/ui/provenance.json` rows are at T3 Code commit `6524a8d4`. Design 59 vendors at `33aa4614`. | New rows cite `33aa4614` with a second archive entry, fetched by `scripts/studio/vendor.mjs fetch`. Existing rows stay at `6524a8d4`. `boundaries.test.ts` accepts all three verdicts. |
| W19 | The desktop already has 8 files adapted from dsh-desktop with no provenance rows. | `apps/desktop-electron/provenance.json` gets rows for them too, at `bc732f5`, with the upstream file's SHA-256 at that commit and the note "adapted before CF1". |
| W20 | `shell.nav.context` (the projects plugin's switcher and its Overview, Environments, Agents and Access links) has no place in D1080's three places. | The projects plugin moves its switcher to `shell.topbar.left` (the project pill). Overview, Agents and Access become `shell.nav.section` pages with `:project` hrefs (`group: 'Project'`). Environments are reachable from the project's Overview and the palette. The slot stays declared, and the shell no longer renders it. |
| W21 | "Branch pill" in the console (no Studio route): the console has environments, not branches. | In Console routes the branch pill is the session's environment switcher. On Studio routes it is Studio's project branch (its existing header context). |

## Execution order

| Milestone | Tasks | Scope (D1086 step) | Runs |
|---|---|---|---|
| — | 0 | Reconcile with the code as built | First |
| **CF1a** — tokens | 1, 2, 3 | Token values, theme selection, monospace and page fixes | After 0, in order. Each task's PR keeps every page usable. |
| **CF1b** — primitives | 4, 5, 6 | The rest of the T3 Code port, overlays, toasts and confirmations | After CF1a, in order (5 rebuilds the wrappers on 4's parts; 6 on 5's dialog). |
| **CF1c** — shell regions | 7, 8, 9, 10, 11, 12 | Frame, inspector and drawer, Studio in the regions, navigation, palette sources, keymap | After CF1b, in order (each builds on the last region). |
| **CF1d** — desktop chrome | 13, 14, 15, 16 | Window chrome, menus, splash and provenance, notifications and the update toast | After 7 (13 needs the top bar), 12 (14 needs the keymap) and 6 (16 needs the toast). 13–15 in order; 16 after 13. |
| **CF1e** — quality gate | 17 | Screenshot tests, axe and the desktop chrome e2e | Last. |

18 tasks in all (0–17). Branches are `cf1<m>-t<N>` (for example `cf1a-t1`), based on `dev`.

## File structure

Paths are from the repository root. **C** means create, **M** means modify, **D** means delete.

```
web/packages/ui/                                   @loams/ui: tokens and primitives (D1073–D1077)
  src/tokens.css                       M  T1   new values, radius scale, new tokens, forced colours
  src/styles.css                       M  T1,4,5  component rules restyled on the tokens
  src/index.ts                         M  T4,5,6  new exports; overlay exports re-pointed
  src/button.tsx                       M  T4   vendored (adapt) from ui/button.tsx; Loams variants kept
  src/badge.tsx                        C  T4   vendored (adapt) from ui/badge.tsx
  src/status.tsx                       M  T4   Badge re-exported from badge.tsx
  src/input.tsx  src/textarea.tsx      C  T4   vendored InputControl and TextareaControl
  src/checkbox.tsx  src/radio-group.tsx  src/number-field.tsx  C  T4
  src/select.tsx  src/combobox.tsx  src/autocomplete.tsx      C  T4   (T5: + SelectMenu and Combobox moved in)
  src/group.tsx  src/separator.tsx  src/alert.tsx             C  T4
  src/empty.tsx  src/table.tsx         M  T4   vendored parts added; Loams Empty/Table kept
  src/collapsible-section-header.tsx   C  T4
  src/field.tsx                        M  T4   Input/Select/Textarea restyled (class names kept)
  src/dialog-styles.ts  src/dialog.tsx  src/alert-dialog.tsx  src/sheet.tsx  C  T5
  src/popover.tsx  src/preview-card.tsx  src/menu.tsx  src/tooltip.tsx      C  T5
  src/tabs.tsx                         C  T5   Loams Tabs moved out of overlay.tsx (not vendored)
  src/overlay.tsx                      D  T5   split into the files above
  src/toast.tsx  src/toast-helpers.ts  src/sidebar-state.ts  C  T6
  src/confirm.tsx                      C  T6   ConfirmDialog and TypedConfirmDialog (Loams)
  src/empty.tsx  src/grain.tsx         M  T4   no grain in Empty (Q929); Grain keeps its API
  provenance.json                      M  T4,5,6  archive 33aa4614 + one row per vendored file
  test/contrast.test.ts                C  T1   WCAG pairs and forced colours (Review Focus 5)
  test/mono.test.ts                    C  T3   monospace only for code, ids, SQL
  test/colours.test.ts                 M  T1   bans text-accent
  test/boundaries.test.ts              M  T4   verdicts as-is|adapt|rewrite
  test/primitives-forms.test.tsx       C  T4
  test/primitives-overlays.test.tsx    C  T5
  test/toast-confirm.test.tsx          C  T6

web/apps/console/
  src/tailwind.css                     M  T1   new colour, radius, text and shadow utilities
  src/cordis/theme.ts                  M  T2   System/Dark/Light before first paint
  src/cordis/main.tsx                  M  T1   Fontsource imports removed
  src/cordis/console.css               M  T1,7,13  fonts; the frame; platform chrome CSS
  src/cordis/fake-desktop.ts           M  T2,13,16  fakes for theme, chrome, update.onState/skip
  src/cordis/desktop.ts                M  T15  reports ready (splash)
  package.json                         M  T1,17  Fontsource removed; test:screens; @axe-core/playwright
  playwright.screens.config.ts         C  T17
  test/screens/shell.screens.spec.ts   C  T17  (+ __screenshots__/ baselines, Q931)
  test/theme.test.ts                   C  T2
  test/tailwind.test.ts                M  T1

web/packages/console-host/
  src/theme.ts                         C  T2   ThemeChoice, resolveDark, ThemeController
  src/mode.ts  src/boot.ts             M  T9   ModeController.prepare; other modes load at boot (W10)
  src/services.ts                      M  T2,8 ThemeService, PanelService, PanelState
  src/index.ts                         M  T2,8
  test/theme.test.ts  test/mode-prepare.test.ts  C  T2,9

web/packages/slots/src/{types,registry}.ts  M  T7,8  new shell.* slots, SlotMeta.routes, PanelTabProps
web/packages/slots/test/slots.test.tsx      M  T7

web/plugins/shell/                    @loams/plugin-shell (D1078–D1082)
  src/layout.tsx                       M  T7   the six-region frame (sidebar/topbar/main/inspector/drawer)
  src/frame.tsx                        C  T7   TopBar, MainRegion, the drag-region markup
  src/nav.ts                           C  T7   inMode, navSections (moved; breaks an import cycle)
  src/sidebar.tsx                      C  T7,10  ShellSidebar: brand, server, threads, Pages, Settings
  src/panel.ts                         C  T8   PanelController (inspector, drawer)
  src/regions.tsx                      C  T8   Inspector and Drawer regions, tab strips
  src/pages.ts                         C  T10  pinned Pages per project, settings classification
  src/palette.tsx                      M  T10,11  shell.palette.source + palette.command
  src/keys.ts                          C  T12  SHELL_KEYMAP, matchShellKey, keyLabel
  src/keyboard.tsx                     C  T12  useShellKeys (one listener)
  src/appearance.tsx                   C  T2   theme and motion menu + settings section
  src/index.tsx                        M  T2,7,8  provides inspector, drawer; injects theme
  package.json                         M  T2,7,8,12  slots, inject, provides, devDeps
  test/frame.test.ts(x)  test/regions.test.tsx  test/pages.test.tsx  C  T7,8,10
  test/keymap.test.ts                  C  T12  (Review Focus 1)
  test/layout.test.tsx                 M  T7   the new chrome

web/plugins/studio/                   Studio moves into the regions (T9)
  src/index.tsx                        M  T9,11,16  registers home, threads, inspector/drawer tabs, topbar
  src/regions.tsx                      C  T9   StudioThreads, StudioInspectorTab, StudioTerminalTab, StudioHome
  src/layout/StudioLayout.tsx          M  T9   main column only
  src/view.tsx                         M  T9   panel and drawer leave the page
  src/sidebar/Sidebar.tsx              M  T3,9  mono; show-more cap; archived moves to Settings
  src/settings/Archived.tsx            C  T9   archived threads (D1079)
  src/palette/commands.ts              M  T12  shell commands routed to Studio
  src/notifications/data.ts            M  T16  onArrive for web OS notifications
  package.json                         M  T9   inject inspector, drawer; the region slots
  test/shell-sidebar.test.tsx          C  T9   (Review Focus 2)
web/packages/studio-ui/src/keybindings/{shared,settings}.ts  M  T12  shell-owned chords removed / listed

web/plugins/projects/src/{index,context}.tsx   M  T10  project pill in shell.topbar.left; project pages
web/plugins/desktop-servers/src/index.tsx      M  T7   server indicator in shell.sidebar.server
web/apps/console/src/hosted/edition.tsx        M  T7   same
web/plugins/agent/src/index.tsx                M  T8   the dock is the inspector's "Agent" tab
web/plugins/data-studio/src/index.tsx          M  T11  "Tables" palette source
web/plugins/desktop-settings/src/{index,update-toast}.tsx  M/C  T16  the update toast in shell.overlay (+ test/update-toast.test.tsx); the Settings › Updates card stays
web/packages/platform-web/src/index.ts         M  T2,16  theme service; notify click → route
web/packages/platform-electron/src/index.ts    M  T2,12,14,16  desktop theme store; shell commands; Notification shim
web/packages/charts/src/theme.ts               M  T1   system fonts

apps/desktop-electron/
  src/main/index.ts                    M  T13,14,15,16  window options, theme IPC, menus, splash, notify
  src/main/shell/window-chrome.ts      C  T13  pure: options per OS, chrome colours
  src/main/shell/window-chrome.electron.ts  C  T13  applies chrome, theme IPC, fullscreen push
  src/main/shell/macos-window-backdrop.ts   C  T13  adapted from dsh-desktop
  src/main/shell/window-state.ts       M  T13  MIN_HEIGHT 640
  src/main/shell/menu-model.ts         M  T14  App/File/Edit/View/Window/Help (D1081 accelerators)
  src/main/shell/context-menu-template.ts   C  T14  adapted from dsh-desktop
  src/main/shell/context-menu.electron.ts   C  T14  adapted from dsh-desktop
  src/main/shell/splash.ts             C  T15  SplashGate (pure)
  src/main/shell/splash.electron.ts    C  T15  the frameless splash window
  src/main/shell/theme-ipc.electron.ts C  T2   theme:set
  src/preload/window-chrome.ts         C  T13  data-platform, data-fullscreen (adapted from dsh-desktop)
  src/preload/notification-shim.ts     C  T16  window.Notification on the shell bridge (adapted from dsh-desktop)
  src/main/update/controller.ts        M  T16  skip a version; automatic vs manual checks
  src/main/update/updater.electron.ts  M  T16  update:skip, scheduler checks are automatic
  src/main/settings.ts                 M  T2,16  ui.theme, update.skippedVersion
  src/preload/index.ts                 M  T2,13,15,16  theme, data-platform/fullscreen, ready, update.onState/skip
  src/shared/contracts.ts              M  T2,13,14,15,16  channels and API additions
  build/splash.html                    C  T15  adapted from dsh-desktop build/splash.html
  build/loams-loader.gif  build/loams-loader-dark.gif  C  T15  generated
  scripts/generate-loader-gifs.mjs     C  T15  adapted from dsh-desktop
  provenance.json  LICENSE.dsh-desktop C  T15
  NOTICE                               M  T15
  electron-builder.config.cjs          M  T15  ship splash and LICENSE.dsh-desktop
  package.json                         M  T15  gifenc devDependency, loader:generate
  test/window-chrome.test.ts           C  T13  (Review Focus 3)
  test/menu-model.test.ts              M  T14
  test/context-menu.test.ts            C  T14
  test/provenance.test.ts  test/splash.test.ts  C  T15
  test/update-skip.test.ts             C  T16
  test/theme-setting.test.ts           C  T2
  test/e2e/windows.ts                  C  T15  mainWindow(app): the console window, not the splash
  test/e2e/chrome.spec.ts              C  T17

NOTICE  scripts/ci/npm-licences.exceptions.json  web/README.md  web/packages/ui/README.md   M  T1  fonts
.github/workflows/ci.yml               M  T17  screenshots and axe in the web job
docs/design/console-ui-stack.md        M  T1   amended tokens table (design 59)
```

## Shared contracts (all tasks use these names)

### Tokens (Task 1)

The existing `--op-*` names keep their meaning. These are added:

| Token | Use |
|---|---|
| `--op-window` | The window behind the sidebar and top bar (light `#f4f2ee`; dark = surface `#111111`). Light `--op-surface` becomes white, the card on the window |
| `--op-input` | Input outlines (W4) |
| `--op-accent-fg` | The accent as text (W2) |
| `--op-warn`, `--op-warn-soft` | Amber state (W3) |
| `--op-row-hover`, `--op-row-selected`, `--op-shadow-chip` | Sidebar rows; the light selected row is a white chip with a 1px shadow |
| `--op-shadow-composer` | `0 12px 28px -18px` (D1073) |
| `--op-dialog-highlight`, `--op-blur`, `--op-glass` | Dialog glass: 14px blur, 80% opacity, 4% white inset in dark |
| `--op-radius-xs` 4, `--op-radius-sm` 6, `--op-radius-md` 8, `--op-radius-lg` 14, `--op-radius-xl` 22; `--op-radius` 10 | D1073 |
| `--op-control-compact` 28, `--op-control` 32, `--op-control-input` 36, `--op-topbar` 44, `--op-row` 28 | D1076 |
| `--op-text-micro` 11, `-small` 12, `-body` 13, `-emphasis` 14, `-section` 16, `-title` 20, `-hero` 24 | D1075 |
| `--op-duration` 120ms, `--op-panel-duration` 0ms (300ms with `html[data-motion="on"]`), `--op-ease-panel` | D1076 |

Tailwind (`web/apps/console/src/tailwind.css`) maps each colour token to `--color-<name>`, the radii to `rounded-xs|sm|md|lg|xl|pill|panel` (`rounded-none` is 0), and the type scale to `text-micro|small|body|emphasis|section|title|hero`.

### Services (`@loams/console-host`)

```ts
// src/theme.ts (Task 2)
export type ThemeChoice = 'system' | 'dark' | 'light';
export const THEME_KEY = 'loams-theme';
export const MOTION_KEY = 'loams.shell.motion';
export function parseThemeChoice(value: unknown): ThemeChoice;
export function resolveDark(choice: ThemeChoice, match?: (query: string) => boolean): boolean;
export function applyTheme(dark: boolean, root?: HTMLElement): void;
export interface ThemeStore { read(): ThemeChoice; write(choice: ThemeChoice, dark: boolean): void }
export function localThemeStore(storage?: Pick<Storage, 'getItem' | 'setItem'>): ThemeStore;
export class ThemeController implements ThemeService { /* see Task 2 */ }

// src/services.ts (Tasks 2, 8)
export interface ThemeService {
  readonly choice: ThemeChoice;
  readonly dark: boolean;
  set(choice: ThemeChoice): void;
  subscribe(listener: () => void): () => void;
}
export interface PanelState {
  open: boolean;
  tab: string | undefined;
  props: Readonly<Record<string, unknown>>;
  size: number;
}
export interface PanelService {
  state(): PanelState;
  open(o?: { tab?: string; props?: Record<string, unknown> }): void;
  close(): void;
  toggle(tab?: string): void;
  resize(size: number): void;
  subscribe(listener: () => void): () => void;
}
// Services gains: theme: ThemeService; inspector: PanelService; drawer: PanelService;
```

### Slots (`@loams/slots`, Tasks 7, 8, 10, 11)

```ts
export interface PanelTabProps {
  /** The current route's params. */
  params: Record<string, string>;
  /** What `inspector.open({ tab, props })` passed. */
  props: Readonly<Record<string, unknown>>;
}
// SlotMap additions:
'shell.sidebar.server': { kind: 'list'; props: Record<string, never> };
'shell.sidebar.threads': { kind: 'list'; props: Record<string, never> };
'shell.topbar.left': { kind: 'list'; props: Record<string, never> };
'shell.topbar.center': { kind: 'list'; props: Record<string, never> };
'shell.topbar.right': { kind: 'list'; props: Record<string, never> };
'shell.home': { kind: 'single'; props: { params: Record<string, string> } };
'shell.inspector.tab': { kind: 'list'; props: PanelTabProps };
'shell.drawer.tab': { kind: 'list'; props: PanelTabProps };
'shell.palette.source': {
  kind: 'list';
  props: { query: string; publish: (commands: readonly PaletteCommand[]) => void };
};
// SlotMeta gains:
/** Route patterns ("/studio/t/:threadId") where an inspector or drawer tab shows; none = everywhere. */
routes?: readonly string[];
```

A tab entry's `meta.id` is its tab id (`studio.diff`), and `meta.label` is its tab label. `inspector.open({ tab: 'studio.diff' })` selects the entry whose `meta.id` is `studio.diff`.

### The shell (Tasks 7–12)

```ts
// plugins/shell/src/panel.ts
export const INSPECTOR: PanelConfig; // { key: 'loams.shell.inspector', min: 240, max: 640, size: 280 }
export const DRAWER: PanelConfig;    // { key: 'loams.shell.drawer', min: 160, max: 560, size: 280 }
export interface PanelConfig { key: string; min: number; max: number; size: number }
export class PanelController implements PanelService { constructor(config: PanelConfig, storage?: Pick<Storage, 'getItem' | 'setItem'>) }

// plugins/shell/src/frame.tsx
export const SIDEBAR = { key: 'loams.shell.sidebar', min: 200, max: 320, size: 232 } as const;

// plugins/shell/src/pages.ts
export const DEFAULT_PINNED: readonly string[]; // ['data', 'deployments', 'git', 'agents', 'connectors'] (Q932)
export const SETTINGS_GROUPS: readonly string[]; // ['Organisation', 'Operate', 'Instance', 'Settings']
export interface PageEntry { id: string; label: string; href: string; icon?: string; group: string; order: number }
export function pageEntries(sections: SlotEntry<'shell.nav.section'>[], legacy: SlotEntry<'console.nav'>[], project: string | undefined): PageEntry[];
export function pinnedPages(all: readonly PageEntry[], pinned: readonly string[]): PageEntry[];
export function readPinned(project: string, storage?: Pick<Storage, 'getItem'>): string[];
export function writePinned(project: string, ids: readonly string[], storage?: Pick<Storage, 'setItem'>): void;
export function isSettingsPage(entry: Pick<PageEntry, 'group' | 'id'>): boolean;
export function currentProject(path: string, storage?: Pick<Storage, 'getItem'>): string | undefined;

// plugins/shell/src/keys.ts
export type ShellKeyCommand =
  | 'palette.toggle' | 'thread.new' | 'sidebar.toggle' | 'inspector.toggle' | 'drawer.toggle'
  | `thread.jump.${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9}` | 'thread.next'
  | 'navigation.back' | 'navigation.forward' | 'thread.reopen' | 'settings.open' | 'overlay.close';
export interface ShellKeyContext { mac: boolean; terminal: boolean; richText: boolean; editable: boolean }
export interface ShellKeyBinding { key: string; command: ShellKeyCommand; label: string; when?: (c: ShellKeyContext) => boolean }
export const SHELL_KEYMAP: readonly ShellKeyBinding[];
export function matchShellKey(e: Pick<KeyboardEvent, 'key' | 'code' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>, ctx: ShellKeyContext): ShellKeyCommand | undefined;
export function keyLabel(key: string, mac: boolean): string[];
```

Shell commands that Studio carries out travel as `SHELL_COMMAND_EVENT` (`loams:shell-command`) with `detail` set to `chat.new`, `thread.jump.N` or `thread.next`. Studio calls `preventDefault()` when it handled one.

### Desktop contracts (`apps/desktop-electron/src/shared/contracts.ts`, Tasks 2, 13–16)

```ts
CH.themeSet = 'theme:set';             // renderer → main: (choice: ThemeChoice, dark: boolean)
CH.shellFullscreen = 'shell:fullscreen'; // main → renderer push: boolean
CH.shellReady = 'shell:ready';          // renderer → main: the console painted (splash closes)
CH.updateSkip = 'update:skip';          // renderer → main: (version: string)
export type ThemeChoice = 'system' | 'dark' | 'light';
export const THEME_ARG = '--loams-theme=';
// LoamsDesktopApi additions:
theme: { initial: ThemeChoice; set(choice: ThemeChoice, dark: boolean): Promise<void> };
shell.ready(): Promise<void>;
shell.onFullscreen(cb: (fullscreen: boolean) => void): () => void;
update.onState(cb: (s: UpdateState) => void): () => void;
update.skip(version: string): Promise<void>;
// ShellCommand gains: 'palette.toggle' | 'sidebar.toggle' | 'inspector.toggle' | 'drawer.toggle'
//   | 'navigation.back' | 'navigation.forward' | 'settings.open' | 'thread.next' | 'thread.reopen'
```

Settings keys: `ui.theme` (a `ThemeChoice`, default `system`) and `update.skippedVersion` (a string).

---

## CF1a — Tokens

### Task 0: Reconcile with the code as built

**Files:**
- Modify: this plan's "Rulings made during execution" only (rows `T0-*`).

**Interfaces:**
- Consumes: `origin/dev` at the commit the task starts from; the local clones `~/Documents/research/t3code` (33aa4614) and `~/Documents/research/dsh-desktop` (bc732f5), read-only.
- Produces: findings, each with paths and the task that absorbs it. No code.

- [x] **Step 1: Confirm the anchors this plan names still hold**

Run each of these. Record any difference as a `T0-*` row.

```bash
cd ~/Documents/Ostriumlabs/loams-wt/cf1a-t0
git log -1 --format='%H %cd' origin/dev
grep -n "export const MIN_HEIGHT" apps/desktop-electron/src/main/shell/window-state.ts      # expect 600
grep -n "SLOT_KINDS" -A14 web/packages/slots/src/registry.ts | grep -c "'"                  # expect 12 slots
grep -rn "name: 'shell.dock.right'" web/plugins --include='*.tsx'                           # expect plugins/agent only
grep -rnoE "text-accent([^-a-z0-9]|$)" web/plugins web/packages web/apps/console/src --include='*.tsx' --include='*.ts' | wc -l   # expect 33 (T0-2)
grep -rln "Adapted from dataelement/dsh-desktop" apps/desktop-electron --exclude=NOTICE     # expect 8 files (T0-3)
git -C ~/Documents/research/t3code rev-parse HEAD                                          # expect 33aa4614783e856977a5f0b856dc65ebc1ce6794
git -C ~/Documents/research/dsh-desktop rev-parse --short=7 HEAD                            # expect bc732f5
```

- [x] **Step 2: Diff the twelve already-vendored primitives between the two T3 Code commits**

The twelve primitives in `web/packages/ui/provenance.json` came from `6524a8d4`. Design 59 vendors at `33aa4614`. For each row, compare the upstream file at the old commit (recorded `upstreamSha256`) with the file in the local clone:

```bash
cd ~/Documents/Ostriumlabs/loams-wt/cf1a-t0
node -e '
const p=require("./web/packages/ui/provenance.json");
const {createHash}=require("crypto");const fs=require("fs");
for(const r of p.files){const f=process.env.HOME+"/Documents/research/t3code/"+r.upstreamPath;
const now=fs.existsSync(f)?createHash("sha256").update(fs.readFileSync(f)).digest("hex"):"missing";
console.log((now===r.upstreamSha256?"same   ":"changed")+" "+r.upstreamPath);}'
```

Expected: a list of `same` or `changed` rows. A `changed` row is not re-vendored in this plan; record it, since Tasks 4–6 restyle those files on the new tokens anyway.

- [x] **Step 3: Check the dependency ages and licences**

```bash
npm view @axe-core/playwright@4.10.2 time.4.10.2 license
npm view gifenc@1.0.3 time.1.0.3 license
```

Expected: both published more than 14 days before today, MIT or MPL-2.0 (axe-core is MPL-2.0; record it, and confirm `scripts/ci/npm-licences.mjs` allows MPL-2.0 for a devDependency; if not, record the exception Task 17 needs).

- [x] **Step 4: Record and commit**

Add the `T0-*` rows to "Rulings made during execution" (`| T0-1 | Finding | Where | Absorbed by | Ruling |`).

```bash
git add docs/plans/2026-10-17-cf1-console-foundation.md
git commit -s -m "docs(cf1): task 0 reconciliation"
```

---

### Task 1: Token values, the radius scale, system fonts and the contrast test

**Files:**
- Modify: `web/packages/ui/src/tokens.css` (whole file)
- Modify: `web/apps/console/src/tailwind.css:25-63` (the `@theme inline` block)
- Modify: `web/apps/console/src/cordis/main.tsx:15-16` (drop the Fontsource imports)
- Modify: `web/apps/console/src/cordis/console.css:15-16` (drop the Archivo and Martian Mono overrides)
- Modify: `web/apps/console/package.json` (drop `@fontsource-variable/archivo` and `@fontsource-variable/martian-mono`)
- Modify: `web/packages/ui/src/styles.css:513-515` (`.loams-notice-warn`) and add the forced-colours block at the end
- Modify: `web/packages/charts/src/theme.ts:38-39`
- Modify: the 33 `text-accent` sites and the 5 `color: var(--op-accent)` CSS rules (listed by the new test; T0-2)
- Modify: the `rounded-none` sites `plugins/pipelines/src/Palette.tsx:84`, `plugins/studio/src/notifications/NotificationsBell.tsx:57`, `packages/studio-ui/src/approvals/Question.tsx:164,198,221`, `packages/studio-ui/src/groups/DiffMatrix.tsx:92`, `packages/studio-ui/src/markdown/Markdown.tsx:149,335`, `packages/ui/src/skeleton.tsx:11`
- Modify: `NOTICE:40-45`, `scripts/ci/npm-licences.exceptions.json`, `web/README.md:86,90`, `web/packages/ui/README.md` ("Fonts"), `docs/design/console-ui-stack.md` (decision 1)
- Create: `web/packages/ui/test/contrast.test.ts`
- Modify: `web/packages/ui/test/colours.test.ts`, `web/apps/console/test/tailwind.test.ts`, `web/packages/ui/test/studio-primitives.test.tsx:58-61,188-201` (the token scan's radius and palette rules)

**Interfaces:**
- Consumes: nothing new.
- Produces: the tokens in "Shared contracts → Tokens"; the Tailwind utilities `bg-window`, `border-input`, `text-accent-fg`, `text-warn`, `bg-warn-soft`, `bg-row-hover`, `bg-row-selected`, `shadow-chip`, `shadow-composer`, `rounded-xs|sm|md|lg|xl|pill|panel`, `text-micro|small|body|emphasis|section|title|hero`, `duration-op`, `ease-panel`.

- [ ] **Step 1: Write the failing contrast test**

Create `web/packages/ui/test/contrast.test.ts`:

```ts
// @vitest-environment node
// WCAG AA for the token pairs used for text (design 59 §3, §7), read from tokens.css in both
// themes. Alpha colours are composited in sRGB over the surface they sit on, as a browser
// paints them. Also Review Focus 5: forced colours keep borders, focus and status visible.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const read = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const TOKENS = read('../src/tokens.css');
const STYLES = read('../src/styles.css');

type Rgba = [number, number, number, number];

/** The `--op-*` declarations of the first block that opens with `selector {`. */
function block(selector: ':root' | '.dark'): Record<string, string> {
  const start = TOKENS.indexOf(`\n${selector} {`);
  if (start < 0) throw new Error(`no ${selector} block`);
  const end = TOKENS.indexOf('\n}', start);
  const out: Record<string, string> = {};
  for (const m of TOKENS.slice(start, end).matchAll(/(--op-[a-z0-9-]+):\s*([^;]+);/g)) {
    out[m[1] as string] = (m[2] as string).trim();
  }
  return out;
}

const LIGHT = block(':root');
const DARK = { ...LIGHT, ...block('.dark') };

const gamma = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);
const linear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const clamp = (c: number) => Math.min(1, Math.max(0, c));

function oklch(l: number, c: number, h: number): [number, number, number] {
  const a = c * Math.cos((h * Math.PI) / 180);
  const b = c * Math.sin((h * Math.PI) / 180);
  const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
    -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
    -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
  ].map((x) => gamma(clamp(x))) as [number, number, number];
}

function parse(value: string, vars: Record<string, string>): Rgba {
  const v = value.trim();
  const ref = /^var\((--op-[a-z0-9-]+)\)$/.exec(v);
  if (ref) return parse(vars[ref[1] as string] ?? 'missing', vars);
  let m = /^#([0-9a-f]{6})$/i.exec(v);
  if (m) {
    const h = m[1] as string;
    return [0, 2, 4].map((i) => Number.parseInt(h.slice(i, i + 2), 16) / 255).concat(1) as Rgba;
  }
  m = /^rgb\((\d+)\s+(\d+)\s+(\d+)(?:\s*\/\s*([\d.]+))?\)$/.exec(v);
  if (m) return [Number(m[1]) / 255, Number(m[2]) / 255, Number(m[3]) / 255, Number(m[4] ?? 1)];
  m = /^oklch\(([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\)$/.exec(v);
  if (m) return [...oklch(Number(m[1]), Number(m[2]), Number(m[3])), Number(m[4] ?? 1)];
  throw new Error(`cannot parse colour "${value}"`);
}

/** `fg` painted over an opaque `bg`. */
function over(fg: Rgba, bg: Rgba): Rgba {
  const a = fg[3];
  return [0, 1, 2].map((i) => (fg[i] as number) * a + (bg[i] as number) * (1 - a)).concat(1) as Rgba;
}

const luminance = (c: Rgba) =>
  0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2]);
function ratio(a: Rgba, b: Rgba): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

const TEXT = [
  '--op-ink',
  '--op-muted',
  '--op-faint',
  '--op-accent-fg',
  '--op-grow',
  '--op-warn',
  '--op-info',
  '--op-danger',
];
const SURFACES = ['--op-bg', '--op-surface', '--op-window', '--op-raised', '--op-row-selected'];
/** A state colour on its own soft fill. */
const ON_SOFT: [string, string][] = [
  ['--op-grow', '--op-grow-soft'],
  ['--op-warn', '--op-warn-soft'],
  ['--op-info', '--op-info-soft'],
  ['--op-danger', '--op-danger-soft'],
  ['--op-accent-fg', '--op-accent-soft'],
  ['--op-ink', '--op-accent-soft'],
];
/** Text on a filled control. */
const ON_FILL: [string, string][] = [
  ['--op-accent-ink', '--op-accent'],
  ['--op-grow-on', '--op-grow-deep'],
  // The solid destructive button (Task 4) writes the canvas colour on danger.
  ['--op-bg', '--op-danger'],
];

/** Every opaque backdrop a surface can sit on: itself, or over the canvas and the surface. */
function backdrops(name: string, vars: Record<string, string>): [string, Rgba][] {
  const c = parse(vars[name] ?? 'missing', vars);
  if (c[3] === 1) return [[name, c]];
  const bg = parse(vars['--op-bg'] as string, vars);
  const surface = parse(vars['--op-surface'] as string, vars);
  return [
    [`${name} on --op-bg`, over(c, bg)],
    [`${name} on --op-surface`, over(c, surface)],
  ];
}

describe.each([
  ['light', LIGHT],
  ['dark', DARK],
] as const)('%s theme contrast', (_theme, vars) => {
  const text = (t: string) => parse(vars[t] ?? 'missing', vars);

  it.each(TEXT)('%s is AA (4.5:1) on every surface', (t) => {
    const failures = SURFACES.flatMap((s) =>
      backdrops(s, vars)
        .map(([label, bg]) => [label, ratio(over(text(t), bg), bg)] as const)
        .filter(([, r]) => r < 4.5)
        .map(([label, r]) => `${label}: ${r.toFixed(2)}`),
    );
    expect(failures).toEqual([]);
  });

  it.each(ON_SOFT)('%s is AA on %s', (t, soft) => {
    for (const [label, bg] of backdrops(soft, vars)) {
      expect(ratio(text(t), bg), label).toBeGreaterThanOrEqual(4.5);
    }
  });

  it.each(ON_FILL)('%s is AA on %s', (t, fill) => {
    expect(ratio(text(t), parse(vars[fill] as string, vars))).toBeGreaterThanOrEqual(4.5);
  });

  it('the focus ring (the accent) is 3:1 against every surface', () => {
    for (const s of ['--op-bg', '--op-surface', '--op-window']) {
      expect(ratio(text('--op-accent'), parse(vars[s] as string, vars)), s).toBeGreaterThanOrEqual(3);
    }
  });
});

describe('design 59 values (D1073)', () => {
  it('has the radius scale', () => {
    expect(LIGHT['--op-radius']).toBe('10px');
    expect([
      LIGHT['--op-radius-xs'],
      LIGHT['--op-radius-sm'],
      LIGHT['--op-radius-md'],
      LIGHT['--op-radius-lg'],
      LIGHT['--op-radius-xl'],
      LIGHT['--op-radius-pill'],
    ]).toEqual(['4px', '6px', '8px', '14px', '22px', '999px']);
  });

  it('has the canvas, the window and the indigo accent', () => {
    expect(DARK['--op-bg']).toBe('#0a0a0a');
    expect(DARK['--op-accent']).toBe('oklch(0.571 0.21 264)');
    expect(LIGHT['--op-window']).toBe('#f4f2ee');
    expect(LIGHT['--op-bg']).toBe('#ffffff');
    expect(LIGHT['--op-accent']).toBe('oklch(0.488 0.217 264)');
  });

  it('has the composer shadow', () => {
    expect(LIGHT['--op-shadow-composer']).toMatch(/^0 12px 28px -18px /);
    expect(DARK['--op-shadow-composer']).toMatch(/^0 12px 28px -18px /);
  });

  it('uses the system font stacks (D1075)', () => {
    expect(TOKENS).toContain('--loams-font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, sans-serif;');
    expect(TOKENS).not.toMatch(/Archivo|Martian/);
  });
});

describe('forced colours (Review Focus 5)', () => {
  const forced = (css: string) => {
    const at = css.indexOf('@media (forced-colors: active)');
    expect(at, 'a forced-colors block').toBeGreaterThan(-1);
    return css.slice(at, css.indexOf('\n}\n', at));
  };

  it('maps hairlines and inputs to CanvasText and the accent to Highlight', () => {
    const f = forced(TOKENS);
    for (const t of ['--op-rule', '--op-rule-soft', '--op-input']) expect(f).toContain(`${t}: CanvasText`);
    expect(f).toContain('--op-accent: Highlight');
  });

  it('draws focus as an outline and keeps status dots visible', () => {
    const f = forced(STYLES);
    expect(f).toMatch(/:focus-visible\s*\{[^}]*outline:\s*2px solid Highlight/);
    expect(f).toMatch(/\.loams-status::before[^{]*\{[^}]*forced-color-adjust:\s*none/);
    expect(f).toMatch(/\[data-status-dot\][^{]*\{[^}]*background:\s*CanvasText/);
  });
});
```

- [ ] **Step 2: Extend the colour test and the Tailwind test**

In `web/packages/ui/test/colours.test.ts`, add after the last `it` inside `describe('colour tokens', …)`:

```ts
  it('uses the accent as text only through text-accent-fg (W2)', () => {
    const hits: string[] = [];
    const TEXT_ACCENT = /\btext-accent(?![-\w])/g;
    const CSS_ACCENT = /(?<![-\w])color:\s*var\(--op-accent\)/g;
    for (const root of ROOTS) {
      for (const path of files(root)) {
        const lines = readFileSync(path, 'utf8').split('\n');
        lines.forEach((line, i) => {
          const re = path.endsWith('.css') ? CSS_ACCENT : TEXT_ACCENT;
          for (const m of line.matchAll(re)) hits.push(`${relative(WEB, path)}:${i + 1}: ${m[0]}`);
        });
      }
    }
    expect(hits).toEqual([]);
  });
```

In `web/apps/console/test/tailwind.test.ts`, replace the "maps fonts and easing" test and add a radius test:

```ts
  it('maps fonts, easing, radii and the type scale to the tokens', () => {
    expect(tailwind).toContain('--font-sans: var(--loams-font-sans);');
    expect(tailwind).toContain('--font-mono: var(--loams-font-mono);');
    expect(tailwind).toContain('--ease-op: var(--op-ease);');
    expect(tailwind).toContain('--ease-panel: var(--op-ease-panel);');
    expect(tailwind).toContain('--radius-none: 0px;');
    for (const step of ['xs', 'sm', 'md', 'lg', 'xl', 'pill']) {
      expect(tailwind).toContain(`--radius-${step}: var(--op-radius-${step});`);
    }
    expect(tailwind).toContain('--radius-panel: var(--op-radius);');
    for (const size of ['micro', 'small', 'body', 'emphasis', 'section', 'title', 'hero']) {
      expect(tailwind).toContain(`--text-${size}: var(--op-text-${size});`);
    }
  });
```

The test's `colorTokens` regex (`/^\s*(--op-[a-z-]+):\s*#/gm`) finds only the hex tokens in the light block. Widen it to every colour form so the new tokens must be mapped too:

```ts
const colorTokens = [...lightBlock.matchAll(/^\s*(--op-[a-z-]+):\s*(?:#|rgb\(|oklch\()/gm)]
  .map((m) => m[1])
  .filter((name) => !name?.startsWith('--op-term-') && !name?.startsWith('--op-chart-'));
```

In `web/packages/ui/test/studio-primitives.test.tsx`, the `renders_with_tokens_only` scan allowed only hard corners and pills, and it treated `border-input` as T3 Code's name. Change both rules to the new scale (Tasks 4–6 add their primitives to its `SPECIMENS`):

```ts
const PALETTE =
  /^(?:[a-z-]+:)*(?:bg|text|border|ring|fill|stroke|outline|shadow|from|to|via|divide|accent|caret|decoration)-(?:(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d|white$|black$|foreground|background|primary|secondary|popover|sidebar|destructive|muted-foreground|placeholder|success|warning|card)/;
/** Corners come from the scale (D1073): none, xs, sm, md, lg, xl, pill, panel. */
const RADIUS = /^(?:[a-z-]+:)*rounded(?:-[a-z]+)?-(?:2xl|3xl|full)$|^rounded$/;
```

and its self-test:

```ts
    div.innerHTML =
      '<span class="bg-red-500 rounded-2xl text-foreground hover:bg-[#fff] rounded-full"></span>' +
      '<span class="bg-surface rounded-pill rounded-md border-input rounded-panel text-muted"></span>';
    expect(offTokenClasses(div)).toEqual([
      'bg-red-500',
      'rounded-2xl',
      'text-foreground',
      'hover:bg-[#fff]',
      'rounded-full',
    ]);
```

- [ ] **Step 3: Run the tests and watch them fail**

```bash
pnpm --filter @loams/ui exec vitest run test/contrast.test.ts test/colours.test.ts
pnpm --filter @loams/console exec vitest run test/tailwind.test.ts
```

Expected: FAIL. `contrast.test.ts` fails "has the radius scale" (`--op-radius` is `0`), the accent values, and "forced colours" (`a forced-colors block: expected -1 to be greater than -1`). `colours.test.ts` lists 38 hits: 33 `text-accent` and 5 `color: var(--op-accent)` (T0-2). `tailwind.test.ts` fails on `--radius-none: 0px;`.

- [ ] **Step 4: Write the tokens**

Replace `web/packages/ui/src/tokens.css` with the following. The chart and terminal blocks keep their current values except `--op-term-selection`, which follows the accent.

```css
/*
 * Loams tokens (design 59, D1073–D1076). Dark is direction B: T3 Code's neutral-black canvas,
 * focused and dense. Light is direction A: a warm window with the work area as a white card.
 * Indigo is the one accent: the primary action, selection and focus (as text it is
 * --op-accent-fg). Emerald (grow), amber (warn), red (danger) and blue (info) mean state only.
 * Put the class `dark` on <html> for dark mode.
 *
 * Values derive from T3 Code apps/web/src/index.css at 33aa4614 (MIT; see LICENSE.t3code).
 * Every colour, shadow and radius in web/plugins and web/packages comes from here
 * (docs/design/console-ui-stack.md): test/colours.test.ts fails on a raw colour literal
 * anywhere else, and test/contrast.test.ts checks every text pair for WCAG AA.
 */
:root {
  --op-window: #f4f2ee;
  --op-bg: #ffffff;
  /* Cards, popovers and dialogs: white on the window in direction A. */
  --op-surface: #ffffff;
  --op-raised: #f1efea;
  --op-rule: #ece8e2;
  --op-rule-soft: #efece7;
  --op-input: #dcd7cf;
  --op-ink: #1d1b18;
  --op-muted: #6b665e;
  --op-faint: #716c64;
  --op-accent: oklch(0.488 0.217 264);
  --op-accent-fg: oklch(0.488 0.217 264);
  --op-accent-soft: oklch(0.488 0.217 264 / 0.1);
  --op-accent-ink: #ffffff;
  --op-grow: #047857;
  --op-grow-soft: rgb(4 120 87 / 0.1);
  --op-grow-deep: #047857;
  --op-grow-on: #ffffff;
  --op-warn: #92400e;
  --op-warn-soft: rgb(146 64 14 / 0.1);
  --op-info: #1d4ed8;
  --op-info-soft: rgb(29 78 216 / 0.1);
  --op-danger: #b91c1c;
  --op-danger-soft: rgb(185 28 28 / 0.1);

  /* Sidebar rows: the selected row is a white chip with a 1px shadow (direction A). */
  --op-row-hover: rgb(29 27 24 / 0.04);
  --op-row-selected: #ffffff;
  --op-shadow-chip: 0 1px 1px rgb(29 27 24 / 0.08), 0 0 0 1px rgb(29 27 24 / 0.04);

  /* Elevation: lightness steps and hairlines; shadows only for floating things. */
  --op-shadow-pop: 0 12px 32px -12px rgb(29 27 24 / 0.18);
  --op-shadow-modal: 0 24px 56px -20px rgb(29 27 24 / 0.28);
  --op-shadow-composer: 0 12px 28px -18px rgb(0 0 0 / 0.4);
  --op-scrim: rgb(29 27 24 / 0.24);
  /* Dialog glass: 80% opacity over a 14px blur; dark adds a 4% white inset highlight. */
  --op-glass: 80%;
  --op-blur: 14px;
  --op-dialog-highlight: inset 0 0 0 0 transparent;

  /* Chart series, the validated categorical order (fixed, never cycled). */
  --op-chart-1: #2a78d6;
  --op-chart-2: #eb6834;
  --op-chart-3: #1baf7a;
  --op-chart-4: #eda100;
  --op-chart-5: #e87ba4;
  --op-chart-6: #008300;
  --op-chart-7: #6250d6;
  --op-chart-8: #e34948;

  /* Terminals (Studio, SD1 Task 22): unchanged except the selection, which follows the accent. */
  --op-term-bg: var(--op-surface);
  --op-term-fg: var(--op-ink);
  --op-term-cursor: var(--op-accent);
  --op-term-selection: rgb(29 78 216 / 0.25);
  --op-term-black: #1a1511;
  --op-term-red: #a63a20;
  --op-term-green: #3f7426;
  --op-term-yellow: #7f4c09;
  --op-term-blue: #2b5c96;
  --op-term-magenta: #85397f;
  --op-term-cyan: #2c6670;
  --op-term-white: #5b5044;
  --op-term-bright-black: #74695b;
  --op-term-bright-red: #b8452a;
  --op-term-bright-green: #4a7f2e;
  --op-term-bright-yellow: #8a520a;
  --op-term-bright-blue: #3667a8;
  --op-term-bright-magenta: #94468e;
  --op-term-bright-cyan: #33737e;
  --op-term-bright-white: #74695b;

  /* Spacing steps of 4px; the radius scale (D1073). */
  --op-space: 4px;
  --op-radius: 10px;
  --op-radius-xs: 4px;
  --op-radius-sm: 6px;
  --op-radius-md: 8px;
  --op-radius-lg: 14px;
  --op-radius-xl: 22px;
  --op-radius-pill: 999px;

  /* Density (D1076). */
  --op-control-compact: 28px;
  --op-control: 32px;
  --op-control-input: 36px;
  --op-topbar: 44px;
  --op-row: 28px;

  /* Type (D1075): system faces; monospace for code, ids and SQL only. */
  --loams-font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, sans-serif;
  --loams-font-mono:
    ui-monospace, "SF Mono", "SFMono-Regular", Menlo, Consolas, "Liberation Mono", monospace;
  --op-text-micro: 11px;
  --op-text-small: 12px;
  --op-text-body: 13px;
  --op-text-emphasis: 14px;
  --op-text-section: 16px;
  --op-text-title: 20px;
  --op-text-hero: 24px;

  /* Motion (D1076): off by default, panels snap; colour and opacity changes are quick. */
  --op-ease: cubic-bezier(0.22, 1, 0.36, 1);
  --op-ease-panel: cubic-bezier(0.32, 0.72, 0, 1);
  --op-duration: 120ms;
  --op-panel-duration: 0ms;
  color-scheme: light;
}

.dark {
  --op-window: #111111;
  --op-bg: #0a0a0a;
  --op-surface: #111111;
  --op-raised: rgb(255 255 255 / 0.04);
  --op-rule: rgb(255 255 255 / 0.06);
  --op-rule-soft: rgb(255 255 255 / 0.04);
  --op-input: rgb(255 255 255 / 0.08);
  --op-ink: #f5f5f5;
  --op-muted: #a3a3a3;
  --op-faint: #8a8a8a;
  --op-accent: oklch(0.571 0.21 264);
  --op-accent-fg: oklch(0.72 0.14 264);
  --op-accent-soft: oklch(0.571 0.21 264 / 0.18);
  --op-accent-ink: #ffffff;
  --op-grow: #34d399;
  --op-grow-soft: rgb(52 211 153 / 0.14);
  --op-grow-deep: #047857;
  --op-grow-on: #ffffff;
  --op-warn: #fbbf24;
  --op-warn-soft: rgb(251 191 36 / 0.14);
  --op-info: #60a5fa;
  --op-info-soft: rgb(96 165 250 / 0.14);
  --op-danger: #f87171;
  --op-danger-soft: rgb(248 113 113 / 0.14);
  --op-row-hover: rgb(255 255 255 / 0.04);
  --op-row-selected: rgb(255 255 255 / 0.07);
  --op-shadow-chip: 0 0 0 0 transparent;
  --op-shadow-pop: 0 12px 32px -12px rgb(0 0 0 / 0.6);
  --op-shadow-modal: 0 24px 56px -20px rgb(0 0 0 / 0.7);
  --op-shadow-composer: 0 12px 28px -18px rgb(0 0 0 / 0.75);
  --op-scrim: rgb(0 0 0 / 0.5);
  --op-dialog-highlight: inset 0 1px 0 rgb(255 255 255 / 0.04);
  /* The same hues, stepped for the dark surface. */
  --op-chart-1: #3987e5;
  --op-chart-2: #d95926;
  --op-chart-3: #199e70;
  --op-chart-4: #c98500;
  --op-chart-5: #d55181;
  --op-chart-6: #008300;
  --op-chart-7: #9085e9;
  --op-chart-8: #e66767;
  --op-term-selection: rgb(119 162 252 / 0.25);
  --op-term-black: #4a4038;
  --op-term-red: #e0664a;
  --op-term-green: #9cc25e;
  --op-term-yellow: #e4a24a;
  --op-term-blue: #7fa9df;
  --op-term-magenta: #cc93cf;
  --op-term-cyan: #72bfc6;
  --op-term-white: #d9d4c8;
  --op-term-bright-black: #938775;
  --op-term-bright-red: #f08a70;
  --op-term-bright-green: #b5d980;
  --op-term-bright-yellow: #f0c070;
  --op-term-bright-blue: #a2c3ef;
  --op-term-bright-magenta: #dfb0e1;
  --op-term-bright-cyan: #98d8de;
  --op-term-bright-white: #f5f5f5;
  color-scheme: dark;
}

/* The optional panel easing (D1076): Settings › Appearance › Motion sets data-motion="on". */
html[data-motion="on"] {
  --op-panel-duration: 300ms;
}

@media (prefers-reduced-motion: reduce) {
  :root,
  html[data-motion="on"] {
    --op-panel-duration: 0ms;
    --op-duration: 0ms;
  }
}

/* Windows High Contrast and other forced palettes (Review Focus 5): alpha hairlines and
   soft fills would vanish, so they become system colours. */
@media (forced-colors: active) {
  :root,
  .dark {
    --op-rule: CanvasText;
    --op-rule-soft: CanvasText;
    --op-input: CanvasText;
    --op-accent: Highlight;
    --op-accent-fg: LinkText;
    --op-row-selected: Highlight;
  }
}
```

The contrast test's `block(':root')` finds the first `\n:root {`. The `@media` blocks open with `:root,`, so they do not match. Confirm that `contrast.test.ts` reads the right block by running it.

- [ ] **Step 5: Map the tokens in Tailwind**

In `web/apps/console/src/tailwind.css`, replace the `@theme inline { … }` block with:

```css
@theme inline {
  --color-*: initial;
  --color-window: var(--op-window);
  --color-bg: var(--op-bg);
  --color-surface: var(--op-surface);
  --color-raised: var(--op-raised);
  --color-rule: var(--op-rule);
  --color-rule-soft: var(--op-rule-soft);
  --color-input: var(--op-input);
  --color-ink: var(--op-ink);
  --color-muted: var(--op-muted);
  --color-faint: var(--op-faint);
  --color-accent: var(--op-accent);
  --color-accent-fg: var(--op-accent-fg);
  --color-accent-soft: var(--op-accent-soft);
  --color-accent-ink: var(--op-accent-ink);
  --color-grow: var(--op-grow);
  --color-grow-soft: var(--op-grow-soft);
  --color-grow-deep: var(--op-grow-deep);
  --color-grow-on: var(--op-grow-on);
  --color-warn: var(--op-warn);
  --color-warn-soft: var(--op-warn-soft);
  --color-info: var(--op-info);
  --color-info-soft: var(--op-info-soft);
  --color-danger: var(--op-danger);
  --color-danger-soft: var(--op-danger-soft);
  --color-row-hover: var(--op-row-hover);
  --color-row-selected: var(--op-row-selected);
  --color-scrim: var(--op-scrim);
  --color-transparent: transparent;
  --color-current: currentColor;

  --font-sans: var(--loams-font-sans);
  --font-mono: var(--loams-font-mono);
  --ease-op: var(--op-ease);
  --ease-panel: var(--op-ease-panel);

  /* The type scale (D1075): text-micro … text-hero. */
  --text-micro: var(--op-text-micro);
  --text-micro--line-height: 16px;
  --text-small: var(--op-text-small);
  --text-small--line-height: 16px;
  --text-body: var(--op-text-body);
  --text-body--line-height: 20px;
  --text-emphasis: var(--op-text-emphasis);
  --text-emphasis--line-height: 20px;
  --text-section: var(--op-text-section);
  --text-section--line-height: 24px;
  --text-title: var(--op-text-title);
  --text-title--line-height: 28px;
  --text-title--letter-spacing: -0.02em;
  --text-hero: var(--op-text-hero);
  --text-hero--line-height: 32px;

  /* Spacing, corners and shadows are tokens too (D1073). rounded-none is a real 0
     (W5); rounded-panel is the panel corner. */
  --spacing: var(--op-space);
  --radius-*: initial;
  --radius-none: 0px;
  --radius-xs: var(--op-radius-xs);
  --radius-sm: var(--op-radius-sm);
  --radius-md: var(--op-radius-md);
  --radius-lg: var(--op-radius-lg);
  --radius-xl: var(--op-radius-xl);
  --radius-pill: var(--op-radius-pill);
  --radius-panel: var(--op-radius);
  --shadow-*: initial;
  --shadow-pop: var(--op-shadow-pop);
  --shadow-modal: var(--op-shadow-modal);
  --shadow-composer: var(--op-shadow-composer);
  --shadow-chip: var(--op-shadow-chip);
}
```

- [ ] **Step 6: Swap the fonts, the warning colour and the forced-colour rules**

1. Delete lines 15–16 of `web/apps/console/src/cordis/main.tsx` (`import '@fontsource-variable/archivo/standard.css';` and `import '@fontsource-variable/martian-mono';`).
2. Delete the two `--loams-font-*` declarations at `web/apps/console/src/cordis/console.css:15-16`.
3. Remove both Fontsource packages from `web/apps/console/package.json` and run `pnpm install` (the lockfile changes).
4. Set `scripts/ci/npm-licences.exceptions.json` to `[]`.
5. In `web/packages/charts/src/theme.ts`, read the stacks from the tokens:

   ```ts
   const FONT_SANS_FALLBACK = '-apple-system, BlinkMacSystemFont, "Segoe UI", Inter, sans-serif';
   const FONT_MONO_FALLBACK = 'ui-monospace, "SF Mono", Menlo, Consolas, monospace';
   ```

   In `readChartTheme`, use `v('--loams-font-sans') || FONT_SANS_FALLBACK` and `v('--loams-font-mono') || FONT_MONO_FALLBACK` where `FONT_SANS`/`FONT_MONO` were used.
6. In `NOTICE`, replace the Archivo/Martian Mono paragraph (lines 40–45) with:

   ```
   The Loams wordmark (web/packages/ui/src/logo.tsx) is outlined from Archivo
   (https://github.com/Omnibus-Type/Archivo), Copyright 2020 The Archivo Project
   Authors, SIL Open Font License, Version 1.1 (https://openfontlicense.org). The
   console no longer bundles any font (design 59, D1075).
   ```

7. In `web/packages/ui/src/styles.css`, change `.loams-notice-warn` to `border-left-color: var(--op-warn);`. Then append:

   ```css
   /* Forced colours (Review Focus 5): focus is an outline, not a shadow; status dots keep
      a system colour and their text label. */
   @media (forced-colors: active) {
     :focus-visible {
       outline: 2px solid Highlight;
       outline-offset: 2px;
     }
     .loams-status::before {
       forced-color-adjust: none;
       background: CanvasText;
     }
     [data-status-dot] {
       forced-color-adjust: none;
       background: CanvasText;
     }
     .loams-btn {
       border-color: ButtonText;
     }
   }
   ```

8. In `web/packages/studio-ui/src/thread-list/StatusIndicators.tsx`, add `data-status-dot=""` to both dot `<span>`s in `ThreadStatusLabel` (the `size-[9px] rounded-full` and the `h-1.5 w-1.5 rounded-full` spans).
9. Update `web/README.md:86,90`, `web/packages/ui/README.md` ("Fonts" now reads: "The tokens use the system UI and monospace stacks (design 59, D1075); no font is bundled. Monospace is for code, ids and SQL only.") and decision 1 of `docs/design/console-ui-stack.md` (append: "Design 59 (D1073) set the values: dark is direction B, light direction A, the radius scale `--op-radius-xs`…`-xl`, and new tokens `--op-window`, `--op-input`, `--op-accent-fg`, `--op-warn(-soft)`, `--op-row-*`, `--op-shadow-chip`, `--op-shadow-composer`; `web/packages/ui/test/contrast.test.ts` checks every text pair for AA.").

- [ ] **Step 7: Move the accent-as-text and square-corner sites**

```bash
cd web
grep -rln "\btext-accent\b" plugins packages apps/console/src --include='*.tsx' --include='*.ts' \
  | xargs sed -i -E 's/\btext-accent([^-a-zA-Z0-9_]|$)/text-accent-fg\1/g'
grep -rn "color: var(--op-accent)" plugins packages apps/console/src --include='*.css'
```

For each CSS hit, change `color: var(--op-accent)` to `color: var(--op-accent-fg)`. Leave `background`, `border` and `outline` uses of `--op-accent` as they are. Then make these corner changes (W5):

| Site | From | To |
|---|---|---|
| `plugins/pipelines/src/Palette.tsx:84` | `rounded-none` | `rounded-md` |
| `plugins/studio/src/notifications/NotificationsBell.tsx:57` | `rounded-none` | `rounded-md` |
| `packages/studio-ui/src/approvals/Question.tsx:164,198,221` | `rounded-none` | `rounded-md` |
| `packages/studio-ui/src/groups/DiffMatrix.tsx:92` | `rounded-none` | `rounded-sm` |
| `packages/studio-ui/src/markdown/Markdown.tsx:149` | `rounded-none` | `rounded-lg` |
| `packages/studio-ui/src/markdown/Markdown.tsx:335` | `rounded-none` | `rounded-xs` |
| `packages/ui/src/skeleton.tsx:11` | `block: 'rounded-none'` | `block: 'rounded-md'` |

`packages/studio-ui/src/chat/rows/WorkLog.tsx:86` keeps `rounded-none`, because it is a text button with no box.

- [ ] **Step 8: Run the tests**

```bash
pnpm --filter @loams/ui test
pnpm --filter @loams/console exec vitest run test/tailwind.test.ts
pnpm --filter @loams/studio-ui test
pnpm --filter @loams/charts test
node --test scripts/ci/npm-licences.test.mjs
```

Expected: PASS. If a contrast pair fails, change only the failing token's value in `tokens.css`, and record the change as a ruling.

- [ ] **Step 9: Look at it**

```bash
pnpm --filter @loams/console dev
```

Open `http://127.0.0.1:5173/ui/?demo`. Toggle the theme from the header button and look at the Overview, Data, SQL, Postgres and Settings pages in both themes. Every page must stay usable. Corners, colours and fonts change, but the layout does not (the layout changes in Task 7).

- [ ] **Step 10: Commit**

```bash
git add -A web/packages/ui web/apps/console web/packages/charts web/packages/studio-ui web/plugins NOTICE \
  scripts/ci/npm-licences.exceptions.json web/README.md docs/design/console-ui-stack.md pnpm-lock.yaml
git commit -s -m "feat(ui): design 59 token values, radius scale and system fonts (CF1 task 1)"
```

---

### Task 2: Theme selection: System, Dark or Light, saved per device

**Files:**
- Create: `web/packages/console-host/src/theme.ts`
- Modify: `web/packages/console-host/src/services.ts` (add `ThemeService`; `Services.theme`), `src/index.ts` (export), `package.json` (`"./theme": "./src/theme.ts"` in `exports`)
- Create: `web/packages/console-host/test/theme.test.ts`
- Modify: `web/apps/console/src/cordis/theme.ts` (whole file)
- Create: `web/apps/console/test/theme.test.ts`
- Modify: `web/packages/platform-web/src/index.ts` (provide `theme`)
- Modify: `web/packages/platform-electron/src/index.ts` (provide `theme` from the desktop store)
- Modify: `web/apps/console/src/cordis/fake-desktop.ts` (the `theme` namespace)
- Create: `web/plugins/shell/src/appearance.tsx`
- Modify: `web/plugins/shell/src/layout.tsx:365-380` (`useTheme` removed), the header's theme button (`layout.tsx:791-805`) and the palette's `theme` command (`layout.tsx:617-624`)
- Modify: `web/plugins/shell/src/index.tsx` and `web/plugins/shell/package.json` (inject `theme`; register the Appearance settings section)
- Modify: `web/plugins/shell/test/layout.test.tsx:100-110` (the old light-default toggle test)
- Modify: `apps/desktop-electron/src/shared/contracts.ts`, `src/shared/version.ts` (`themeFromArgv`), `src/preload/index.ts`, `src/main/settings.ts`, `src/main/index.ts:142-158`
- Create: `apps/desktop-electron/src/main/shell/theme-ipc.electron.ts`
- Create: `apps/desktop-electron/test/theme-setting.test.ts`

**Interfaces:**
- Consumes: the tokens from Task 1 (`.dark`).
- Produces: `ThemeChoice`, `THEME_KEY`, `MOTION_KEY`, `parseThemeChoice`, `resolveDark`, `applyTheme`, `ThemeStore`, `localThemeStore`, `ThemeController` and `ThemeService` (the `theme` service). From the shell: `AppearanceMenu({ theme }: { theme: ThemeService })`, `AppearanceSection({ theme }: { theme: ThemeService })`, `readMotion(): boolean` and `setMotion(on: boolean): void`. From the desktop: `CH.themeSet`, `THEME_ARG`, `themeFromArgv(argv): ThemeChoice`, `readThemeChoice(file): ThemeChoice`, `LoamsDesktopApi.theme`.

- [ ] **Step 1: Write the failing tests**

`web/packages/console-host/test/theme.test.ts`:

```ts
import { describe, expect, it, vi } from 'vitest';
import {
  localThemeStore,
  parseThemeChoice,
  resolveDark,
  ThemeController,
  type ThemeStore,
} from '../src/theme.js';

/** A matchMedia whose light/dark answer can be flipped, with change listeners. */
function os(prefers: 'light' | 'dark' | 'none') {
  const listeners = new Set<() => void>();
  let state = prefers;
  const list = {
    get matches() {
      return state === 'light';
    },
    addEventListener: (_: string, l: () => void) => listeners.add(l),
    removeEventListener: (_: string, l: () => void) => listeners.delete(l),
  };
  return {
    matchMedia: () => list as unknown as MediaQueryList,
    match: (q: string) => (q.includes('light') ? state === 'light' : state === 'dark'),
    flip(next: 'light' | 'dark') {
      state = next;
      for (const l of listeners) l();
    },
  };
}

function memoryStore(initial = 'system'): ThemeStore & { writes: [string, boolean][] } {
  let value = initial;
  const writes: [string, boolean][] = [];
  return {
    writes,
    read: () => parseThemeChoice(value),
    write: (choice, dark) => {
      value = choice;
      writes.push([choice, dark]);
    },
  };
}

describe('theme selection (D1074)', () => {
  it('follows the OS, and is dark when the OS states no preference', () => {
    expect(resolveDark('system', os('light').match)).toBe(false);
    expect(resolveDark('system', os('dark').match)).toBe(true);
    expect(resolveDark('system', os('none').match)).toBe(true);
    expect(resolveDark('light', os('dark').match)).toBe(false);
    expect(resolveDark('dark', os('light').match)).toBe(true);
  });

  it('reads anything unknown as System', () => {
    expect(parseThemeChoice('dark')).toBe('dark');
    expect(parseThemeChoice('sepia')).toBe('system');
    expect(parseThemeChoice(null)).toBe('system');
  });

  it('toggles .dark on <html>, follows an OS flip while on System, and saves each change', () => {
    const env = os('light');
    const root = document.createElement('html');
    const store = memoryStore();
    const theme = new ThemeController(store, { matchMedia: env.matchMedia, root });
    expect(root.classList.contains('dark')).toBe(false);
    const seen = vi.fn();
    theme.subscribe(seen);
    env.flip('dark');
    expect(theme.dark).toBe(true);
    expect(root.classList.contains('dark')).toBe(true);
    expect(store.writes.at(-1)).toEqual(['system', true]);
    theme.set('light');
    expect(root.classList.contains('dark')).toBe(false);
    expect(store.writes.at(-1)).toEqual(['light', false]);
    env.flip('light');
    env.flip('dark');
    expect(theme.dark).toBe(false); // an explicit choice ignores the OS
    expect(seen).toHaveBeenCalled();
    theme.dispose();
  });

  it('keeps working when storage throws (a private window)', () => {
    const broken = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    };
    const store = localThemeStore(broken);
    expect(store.read()).toBe('system');
    expect(() => store.write('dark', true)).not.toThrow();
  });
});
```

`web/apps/console/test/theme.test.ts`:

```ts
import { afterEach, describe, expect, it } from 'vitest';
import { applySavedTheme, initialChoice } from '../src/cordis/theme.js';

afterEach(() => {
  localStorage.clear();
  delete (globalThis as { loamsDesktop?: unknown }).loamsDesktop;
});

describe('the theme before first paint', () => {
  it('is dark with no saved choice and no OS preference (jsdom has no matchMedia)', () => {
    const root = document.createElement('html');
    applySavedTheme(root);
    expect(root.classList.contains('dark')).toBe(true);
  });

  it('keeps a saved Light', () => {
    localStorage.setItem('loams-theme', 'light');
    const root = document.createElement('html');
    applySavedTheme(root);
    expect(root.classList.contains('dark')).toBe(false);
  });

  it('prefers the desktop setting the preload passed', () => {
    localStorage.setItem('loams-theme', 'dark');
    (globalThis as { loamsDesktop?: unknown }).loamsDesktop = { theme: { initial: 'light' } };
    expect(initialChoice()).toBe('light');
  });

  it('turns panel motion on only when it was chosen', () => {
    const root = document.createElement('html');
    applySavedTheme(root);
    expect(root.dataset.motion).toBeUndefined();
    localStorage.setItem('loams.shell.motion', 'on');
    applySavedTheme(root);
    expect(root.dataset.motion).toBe('on');
  });
});
```

`apps/desktop-electron/test/theme-setting.test.ts`:

```ts
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { readThemeChoice, THEME_SETTING, writeSetting } from "../src/main/settings";
import { THEME_ARG, themeFromArgv } from "../src/shared/version";

let dir: string | undefined;
afterEach(() => dir && rmSync(dir, { recursive: true, force: true }));

describe("the desktop's theme setting (D1074)", () => {
	it("defaults to system and survives a bad value", () => {
		dir = mkdtempSync(join(tmpdir(), "loams-theme-"));
		const file = join(dir, "settings.json");
		expect(readThemeChoice(file)).toBe("system");
		writeSetting(file, THEME_SETTING, "sepia");
		expect(readThemeChoice(file)).toBe("system");
		writeSetting(file, THEME_SETTING, "light");
		expect(readThemeChoice(file)).toBe("light");
		expect(JSON.parse(readFileSync(file, "utf8"))["ui.theme"]).toBe("light");
	});

	it("reaches the preload as an argument", () => {
		expect(themeFromArgv([`${THEME_ARG}dark`])).toBe("dark");
		expect(themeFromArgv([`${THEME_ARG}nonsense`])).toBe("system");
		expect(themeFromArgv([])).toBe("system");
	});
});
```

- [ ] **Step 2: Run them and watch them fail**

```bash
pnpm --filter @loams/console-host exec vitest run test/theme.test.ts
pnpm --filter @loams/console exec vitest run test/theme.test.ts
pnpm --filter @loams/desktop exec vitest run test/theme-setting.test.ts
```

Expected: FAIL. "Cannot find module '../src/theme.js'", "applySavedTheme … expected false to be true" (today the default is light), and "readThemeChoice is not exported".

- [ ] **Step 3: Implement the theme module**

`web/packages/console-host/src/theme.ts`:

```ts
// The console's theme (design 59, D1074): System follows `prefers-color-scheme` and is dark
// when the OS states no preference; Dark and Light are explicit. `.dark` on <html> is the
// switch the tokens read. The choice is per device: local storage on the web, the desktop's
// settings.json on the desktop (the platform supplies the store).

import type { ThemeService } from './services.js';

export type ThemeChoice = 'system' | 'dark' | 'light';
/** The key the original console and the shell's toggle always used. */
export const THEME_KEY = 'loams-theme';
/** "on" turns the optional panel easing on (D1076); anything else leaves panels snapping. */
export const MOTION_KEY = 'loams.shell.motion';

export function parseThemeChoice(value: unknown): ThemeChoice {
  return value === 'dark' || value === 'light' || value === 'system' ? value : 'system';
}

function osMatch(query: string): boolean {
  try {
    return globalThis.matchMedia?.(query).matches === true;
  } catch {
    return false;
  }
}

/** Dark unless the choice is Light, or System and the OS says light. */
export function resolveDark(choice: ThemeChoice, match: (query: string) => boolean = osMatch): boolean {
  if (choice !== 'system') return choice === 'dark';
  return !match('(prefers-color-scheme: light)');
}

export function applyTheme(
  dark: boolean,
  root: HTMLElement | undefined = globalThis.document?.documentElement,
): void {
  root?.classList.toggle('dark', dark);
}

export interface ThemeStore {
  read(): ThemeChoice;
  /** Saves the choice; `dark` is what it resolved to (the desktop's chrome needs it). */
  write(choice: ThemeChoice, dark: boolean): void;
}

function browserStorage(): Pick<Storage, 'getItem' | 'setItem'> | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

export function localThemeStore(
  storage: Pick<Storage, 'getItem' | 'setItem'> | undefined = browserStorage(),
): ThemeStore {
  return {
    read() {
      try {
        return parseThemeChoice(storage?.getItem(THEME_KEY));
      } catch {
        return 'system';
      }
    },
    write(choice) {
      try {
        storage?.setItem(THEME_KEY, choice);
      } catch {
        // Storage blocked (a private window): the choice holds for this page.
      }
    },
  };
}

type MediaLike = Pick<MediaQueryList, 'matches' | 'addEventListener' | 'removeEventListener'>;

export interface ThemeEnv {
  matchMedia?: (query: string) => MediaLike;
  root?: HTMLElement;
}

export class ThemeController implements ThemeService {
  #choice: ThemeChoice;
  #dark: boolean;
  readonly #listeners = new Set<() => void>();
  readonly #media: MediaLike | undefined;
  readonly #onMedia = () => this.#sync(false);

  constructor(
    private readonly store: ThemeStore,
    private readonly env: ThemeEnv = {},
  ) {
    this.#choice = store.read();
    const matchMedia = env.matchMedia ?? ((q: string) => globalThis.matchMedia?.(q));
    try {
      this.#media = matchMedia('(prefers-color-scheme: light)') ?? undefined;
    } catch {
      this.#media = undefined;
    }
    this.#media?.addEventListener?.('change', this.#onMedia);
    this.#dark = this.#resolve();
    applyTheme(this.#dark, env.root);
  }

  get choice(): ThemeChoice {
    return this.#choice;
  }

  get dark(): boolean {
    return this.#dark;
  }

  set(choice: ThemeChoice): void {
    this.#choice = choice;
    this.#sync(true);
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  dispose(): void {
    this.#media?.removeEventListener?.('change', this.#onMedia);
    this.#listeners.clear();
  }

  #resolve(): boolean {
    if (this.#choice !== 'system') return this.#choice === 'dark';
    return !(this.#media?.matches ?? false);
  }

  #sync(chosen: boolean): void {
    const dark = this.#resolve();
    const changed = dark !== this.#dark;
    this.#dark = dark;
    applyTheme(dark, this.env.root);
    // An OS flip under System is saved too: the desktop's native chrome follows it (D1083).
    if (chosen || changed) this.store.write(this.#choice, dark);
    for (const listener of [...this.#listeners]) listener();
  }
}
```

In `services.ts`, add the following (and `theme: ThemeService;` to `Services`):

```ts
/** The theme choice and what it resolves to (design 59, D1074); the platform provides it. */
export interface ThemeService {
  readonly choice: import('./theme.js').ThemeChoice;
  readonly dark: boolean;
  set(choice: import('./theme.js').ThemeChoice): void;
  subscribe(listener: () => void): () => void;
}
```

Export `ThemeService` from `src/index.ts`, plus `* from './theme.js'` (named: `applyTheme`, `localThemeStore`, `MOTION_KEY`, `parseThemeChoice`, `resolveDark`, `THEME_KEY`, `ThemeChoice`, `ThemeController`, `ThemeEnv`, `ThemeStore`).

- [ ] **Step 4: First paint, the platforms and the fake desktop**

Replace `web/apps/console/src/cordis/theme.ts`:

```ts
// The theme before the console renders (design 59, D1074): the desktop's saved choice (the
// preload passes it), else this browser's, else System, which is dark when the OS states no
// preference. main.tsx imports this first; the CSP allows no inline script, so it runs as the
// entry's first module. The `theme` service takes over once the platform plugin is applied.

import {
  applyTheme,
  MOTION_KEY,
  parseThemeChoice,
  resolveDark,
  THEME_KEY,
  type ThemeChoice,
} from '@loams/console-host/theme';

export { THEME_KEY };

export function initialChoice(): ThemeChoice {
  const desktop = (globalThis as { loamsDesktop?: { theme?: { initial?: unknown } } }).loamsDesktop
    ?.theme?.initial;
  if (desktop !== undefined) return parseThemeChoice(desktop);
  try {
    return parseThemeChoice(localStorage.getItem(THEME_KEY));
  } catch {
    return 'system';
  }
}

export function applySavedTheme(root: HTMLElement = document.documentElement): void {
  applyTheme(resolveDark(initialChoice()), root);
  let motion = false;
  try {
    motion = localStorage.getItem(MOTION_KEY) === 'on';
  } catch {
    // Storage blocked: panels snap.
  }
  if (motion) root.dataset.motion = 'on';
  else delete root.dataset.motion;
}

applySavedTheme();
```

In `web/packages/platform-web/src/index.ts`, inside `apply` after `ctx.provide('platform', platform)`:

```ts
      ctx.provide('theme', new ThemeController(localThemeStore()));
```

In `web/packages/platform-electron/src/index.ts`, add the store and provide it the same way:

```ts
import { localThemeStore, parseThemeChoice, ThemeController, type ThemeStore } from '@loams/console-host';

/** The desktop keeps the choice in settings.json (D1074); local storage mirrors it for `?desktop`. */
export function desktopThemeStore(api: LoamsDesktopApi): ThemeStore {
  const local = localThemeStore();
  return {
    read: () => (api.theme ? parseThemeChoice(api.theme.initial) : local.read()),
    write: (choice, dark) => {
      local.write(choice, dark);
      void api.theme?.set(choice, dark);
    },
  };
}
// in createElectronPlatform's apply:  ctx.provide('theme', new ThemeController(desktopThemeStore(api)));
```

In `fake-desktop.ts`, add to the returned api:

```ts
    theme: {
      initial: parseThemeChoice(localStorage.getItem('loams-theme')),
      set: async () => {},
    },
```

- [ ] **Step 5: The shell's appearance controls**

`web/plugins/shell/src/appearance.tsx`:

```tsx
// Appearance (design 59, D1074, D1076): the theme choice (System, Dark, Light) and the
// optional panel motion. The header menu and Settings › Appearance both use it.

import { MOTION_KEY, type ThemeChoice, type ThemeService } from '@loams/console-host';
import { Menu, MenuItem, MenuLabel, MenuSeparator, Segmented, Switch } from '@loams/ui';
import { Check, Monitor, Moon, Sun } from 'lucide-react';
import { useState, useSyncExternalStore } from 'react';

const LABEL: Record<ThemeChoice, string> = { system: 'System', dark: 'Dark', light: 'Light' };
const CHOICES: readonly ThemeChoice[] = ['system', 'dark', 'light'];

export function useThemeState(theme: ThemeService): { choice: ThemeChoice; dark: boolean } {
  const choice = useSyncExternalStore(theme.subscribe.bind(theme), () => theme.choice);
  const dark = useSyncExternalStore(theme.subscribe.bind(theme), () => theme.dark);
  return { choice, dark };
}

export function readMotion(): boolean {
  try {
    return localStorage.getItem(MOTION_KEY) === 'on';
  } catch {
    return false;
  }
}

export function setMotion(on: boolean): void {
  try {
    localStorage.setItem(MOTION_KEY, on ? 'on' : 'off');
  } catch {
    // Storage blocked: the setting holds for this page.
  }
  if (on) document.documentElement.dataset.motion = 'on';
  else delete document.documentElement.dataset.motion;
}

export function AppearanceMenu({ theme }: { theme: ThemeService }) {
  const { choice, dark } = useThemeState(theme);
  const Icon = choice === 'system' ? Monitor : dark ? Moon : Sun;
  return (
    <Menu
      label="Appearance"
      trigger={
        <button type="button" className="lc-round-btn" aria-label={`Theme: ${LABEL[choice]}`}>
          <Icon size={16} aria-hidden="true" />
        </button>
      }
    >
      <MenuLabel>Theme</MenuLabel>
      {CHOICES.map((c) => (
        <MenuItem key={c} onSelect={() => theme.set(c)}>
          <Check size={14} aria-hidden="true" style={{ visibility: c === choice ? 'visible' : 'hidden' }} />
          <span aria-current={c === choice ? 'true' : undefined}>{LABEL[c]}</span>
        </MenuItem>
      ))}
      <MenuSeparator />
      <MenuItem onSelect={() => setMotion(!readMotion())}>
        {readMotion() ? 'Turn panel motion off' : 'Turn panel motion on'}
      </MenuItem>
    </Menu>
  );
}

export function AppearanceSection({ theme }: { theme: ThemeService }) {
  const { choice } = useThemeState(theme);
  const [motion, setMotionState] = useState(readMotion);
  return (
    <section className="grid gap-3" aria-labelledby="lc-appearance">
      <h2 id="lc-appearance" className="m-0 text-section font-semibold">
        Appearance
      </h2>
      <Segmented
        label="Theme"
        value={choice}
        onValueChange={(v) => theme.set(v as ThemeChoice)}
        options={CHOICES.map((c) => ({ value: c, label: LABEL[c] }))}
      />
      <Switch
        checked={motion}
        onCheckedChange={(on) => {
          setMotion(on);
          setMotionState(on);
        }}
        label="Animate panels"
      />
    </section>
  );
}
```

`Segmented` takes `label`, `value`, `onValueChange` and `options`; `Switch` takes `checked`, `onCheckedChange` and `label`; `Menu` takes `trigger`, `label` and children; `MenuItem` takes `onSelect` (as in `web/packages/ui/src` on 2026-10-17).

In `layout.tsx`:
1. Delete `useTheme` and `THEME_KEY`.
2. Add `theme?: ThemeService` to `Layout`'s props, and take `const theme = useMemo(() => props.theme ?? new ThemeController(localThemeStore()), [props.theme]);`.
3. Replace the header's sun/moon button with `<AppearanceMenu theme={theme} />`.
4. Replace the palette's `theme` command with one command per choice:

```ts
    ...(['system', 'dark', 'light'] as const).map((c) => ({
      id: `theme:${c}`,
      title: `Theme: ${c === 'system' ? 'System' : c === 'dark' ? 'Dark' : 'Light'}`,
      group: 'Appearance',
      keywords: ['appearance', 'theme', 'colour', 'dark mode'],
      mode: current,
      run: () => theme.set(c),
    })),
```

In `index.tsx`, add `'theme'` to `inject`, pass `theme={service(ctx, 'theme')}` to `Layout`, and register the settings section:

```tsx
    const theme = service(ctx, 'theme');
    ctx.effect(() =>
      slots.register(
        { name: 'console.settings.section', plugin: 'shell', order: 5, meta: { id: 'appearance', label: 'Appearance' } },
        () => <AppearanceSection theme={theme} />,
      ),
    );
```

In `package.json`, add `"theme"` to `loams.plugin.inject` and `"console.settings.section"` to `loams.plugin.slots`. Re-export `AppearanceMenu`, `AppearanceSection`, `readMotion` and `setMotion` from `src/index.tsx`, and drop the `THEME_KEY` and `useTheme` re-exports.

Replace the old light-default toggle test in `test/layout.test.tsx` with:

```tsx
  it('offers System, Dark and Light and saves the choice', async () => {
    setup('/', flags);
    fireEvent.click(screen.getByRole('button', { name: /^Theme: /i }));
    fireEvent.click(await screen.findByRole('menuitem', { name: /Light/ }));
    expect(localStorage.getItem('loams-theme')).toBe('light');
    expect(document.documentElement.classList.contains('dark')).toBe(false);
  });
```

- [ ] **Step 6: The desktop setting and bridge**

In `apps/desktop-electron/src/shared/contracts.ts`, add `themeSet: "theme:set"` to `CH`, and add:

```ts
export type ThemeChoice = "system" | "dark" | "light";

export interface LoamsDesktopApi {
	/** D1074: the saved choice (passed at window creation) and its setter; `dark` drives the chrome. */
	theme: {
		initial: ThemeChoice;
		set(choice: ThemeChoice, dark: boolean): Promise<void>;
	};
}
```

In `src/shared/version.ts`:

```ts
import type { ThemeChoice } from "./contracts";

export const THEME_ARG = "--loams-theme=";

export function themeFromArgv(argv: readonly string[]): ThemeChoice {
	const v = argv.find((a) => a.startsWith(THEME_ARG))?.slice(THEME_ARG.length);
	return v === "dark" || v === "light" ? v : "system";
}
```

In `src/main/settings.ts`:

```ts
export const THEME_SETTING = "ui.theme";

export function readThemeChoice(file: string): ThemeChoice {
	const v = readSetting<unknown>(file, THEME_SETTING, "system");
	return v === "dark" || v === "light" ? v : "system";
}
```

Create `src/main/shell/theme-ipc.electron.ts`:

```ts
import { ipcMain, nativeTheme } from "electron";
import { CH, type ThemeChoice } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { THEME_SETTING, writeSetting } from "../settings";

/** `theme:set` (D1074): save the choice, and let the OS dialogs and menus follow it. */
export function registerThemeIpc(o: {
	settingsFile: () => string;
	onTheme: (dark: boolean) => void;
}): void {
	ipcMain.handle(CH.themeSet, (e, choice: unknown, dark: unknown) => {
		assertTrustedSender(e);
		if (choice !== "system" && choice !== "dark" && choice !== "light") return;
		if (typeof dark !== "boolean") return;
		writeSetting(o.settingsFile(), THEME_SETTING, choice satisfies ThemeChoice);
		nativeTheme.themeSource = choice;
		o.onTheme(dark);
	});
}
```

In `src/main/index.ts`:
1. Inside `app.whenReady`, before the window is created: `nativeTheme.themeSource = readThemeChoice(settingsFile());` and `registerThemeIpc({ settingsFile, onTheme: () => {} });`. Task 13 gives `onTheme` its body.
2. Add `` `${THEME_ARG}${readThemeChoice(settingsFile())}` `` to `additionalArguments` in `createWindow`.

In `src/preload/index.ts`, add to `api`:

```ts
	theme: {
		initial: themeFromArgv(process.argv),
		set: (choice, dark) => invoke(CH.themeSet, choice, dark),
	},
```

- [ ] **Step 7: Run the tests**

```bash
pnpm --filter @loams/console-host exec vitest run test/theme.test.ts
pnpm --filter @loams/console exec vitest run test/theme.test.ts
pnpm --filter @loams/plugin-shell test
pnpm --filter @loams/desktop exec vitest run test/theme-setting.test.ts
pnpm --filter @loams/desktop typecheck
pnpm --filter @loams/console typecheck
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add web/packages/console-host web/apps/console web/packages/platform-web web/packages/platform-electron \
  web/plugins/shell apps/desktop-electron
git commit -s -m "feat(shell): theme follows the OS, dark by default, chosen per device (CF1 task 2)"
```

---

### Task 3: Monospace only for code, ids and SQL; pages after the token change

**Files:**
- Create: `web/packages/ui/test/mono.test.ts`
- Modify: `web/packages/ui/src/styles.css` (`.loams-status`, `.loams-badge`, `.loams-table th`, `.loams-stat > span`: sans; `.loams-snippet` keeps mono)
- Modify: every hit the test lists. On 2026-10-17 that is about 120 lines in about 60 files; the largest are `plugins/git/src/WorkspaceDetail.tsx` (10), `plugins/durable/src/pages.tsx` (10), `plugins/connectors/src/instances-page.tsx` (9), `plugins/agent/src/panel.tsx` (9), `apps/console/src/cordis/console.css` (8), `plugins/studio/src/sidebar/Sidebar.tsx` (7), `plugins/live/src/pages.tsx` (7), `plugins/git/src/EnvView.tsx` (7) and `packages/admin-ui/src/admin.css` (7).
- Modify: this plan's "Rulings made during execution" (one `T3-*` row per page that needed more than the sweep).

**Interfaces:**
- Consumes: the type scale from Task 1 (`text-micro`, `text-small`).
- Produces: the marker convention `mono-ok: <code|id|sql|path|hash|number>` that later tasks follow.

- [ ] **Step 1: Write the failing test**

`web/packages/ui/test/mono.test.ts`:

```ts
// @vitest-environment node
// D1075: monospace is for code, ids and SQL only, never for labels. Outside @loams/ui's own
// sources, a mono face (`font-mono`, `--loams-font-mono`, `loams-mono`) may appear only on a
// line that says why with `mono-ok: <code|id|sql|path|hash|number>`, or on a <code>, <pre>,
// <kbd> or <samp> element. Inside @loams/ui, the label primitives use the UI face.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const WEB = fileURLToPath(new URL('../../..', import.meta.url));
const ROOTS = ['plugins', 'packages', 'apps/console/src'].map((d) => join(WEB, d));
const UI_SRC = join(WEB, 'packages/ui/src');
const SKIP = new Set(['node_modules', 'dist', 'gen', 'test', 'tests', '__tests__']);
const SOURCE = /\.(css|tsx?|jsx?)$/;
const MONO = /\bfont-mono\b|--loams-font-mono|\bloams-mono\b/;
const MARKED = /mono-ok:\s*(code|id|sql|path|hash|number)\b/;
const ELEMENT = /<(code|pre|kbd|samp)\b/;

function* files(dir: string): Generator<string> {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name) || name.startsWith('.')) continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* files(path);
    else if (SOURCE.test(name) && !/\.(test|spec)\./.test(name)) yield path;
  }
}

export function monoHits(): string[] {
  const hits: string[] = [];
  for (const root of ROOTS) {
    for (const path of files(root)) {
      if (!relative(UI_SRC, path).startsWith('..')) continue;
      readFileSync(path, 'utf8')
        .split('\n')
        .forEach((line, i) => {
          if (MONO.test(line) && !MARKED.test(line) && !ELEMENT.test(line)) {
            hits.push(`${relative(WEB, path)}:${i + 1}`);
          }
        });
    }
  }
  return hits;
}

describe('monospace (D1075)', () => {
  it('the scan catches unmarked use and allows marked use', () => {
    expect(MONO.test("className='font-mono text-faint'")).toBe(true);
    expect(MARKED.test("className={/* mono-ok: id */ 'font-mono'}")).toBe(true);
    expect(MARKED.test("className={/* mono-ok: because */ 'font-mono'}")).toBe(false);
    expect(ELEMENT.test('<code className="font-mono">x</code>')).toBe(true);
  });

  it('labels outside @loams/ui do not use monospace', () => {
    expect(monoHits()).toEqual([]);
  });

  it("@loams/ui's label primitives use the UI face", () => {
    const css = readFileSync(join(UI_SRC, 'styles.css'), 'utf8');
    for (const selector of ['.loams-status {', '.loams-badge {', '.loams-table th {', '.loams-stat > span {']) {
      const at = css.indexOf(selector);
      expect(at, selector).toBeGreaterThan(-1);
      expect(css.slice(at, css.indexOf('}', at)), selector).not.toContain('--loams-font-mono');
    }
    const snippet = css.slice(css.indexOf('.loams-snippet {'));
    expect(snippet.slice(0, snippet.indexOf('}'))).toContain('--loams-font-mono');
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `pnpm --filter @loams/ui exec vitest run test/mono.test.ts`
Expected: FAIL. "labels outside @loams/ui do not use monospace" lists about 120 `path:line` hits, and the `.loams-status {` check fails.

- [ ] **Step 3: Sweep**

For every hit, decide what the text is:
- **A label** (section headings, column headers, "Pinned", status words, badges, counts shown as words, breadcrumbs): remove `font-mono`. Where a heading used `uppercase tracking-[0.12em]` with mono, use `text-micro font-medium text-faint` instead (sentence case).
- **A number** (an elapsed time, a count, a size): replace `font-mono` with `tabular-nums`.
- **Code, an id, SQL, a path or a hash**: keep mono and add the marker on that line, for example `className={/* mono-ok: id */ 'font-mono text-small text-faint'}`. In CSS, use `font-family: var(--loams-font-mono); /* mono-ok: sql */`.

`web/apps/console/src/tailwind.css`'s `--font-mono: var(--loams-font-mono);` mapping is a hit too: append `/* mono-ok: code */` to that line (the `font-mono` utility exists for code). In `styles.css`, set `.loams-status`, `.loams-badge`, `.loams-table th` and `.loams-stat > span` to `font-family: inherit;`, and drop their letter-spacing and uppercase. Keep `.loams-snippet` mono.

Studio's sidebar examples (`plugins/studio/src/sidebar/Sidebar.tsx`):
- `WorkingDuration` becomes `'shrink-0 tabular-nums text-micro text-faint'`.
- The "Pinned" marker becomes `'block px-2 pt-1 text-micro font-medium text-faint'`.
- The branch name keeps mono with `mono-ok: id`.

- [ ] **Step 4: Walk every page in both themes**

```bash
pnpm --filter @loams/console dev
```

Open `/ui/?demo` and `/ui/?desktop`. In both themes, visit every route in the nav list of `web/plugins/shell/src/layout.tsx`'s `SECTIONS` and each page the table in this plan's Task 10 names. A page "looks wrong" (D1086) when one of these holds: text fails to read; a control loses its outline; corners mix square and round in one group; or a page relied on an ochre accent for meaning. Fix it in this task with tokens only. Add one `T3-*` ruling row per page fixed, naming the file and the change.

- [ ] **Step 5: Run the tests**

```bash
pnpm --filter @loams/ui test
pnpm -C web -r --if-present test
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A web docs/plans/2026-10-17-cf1-console-foundation.md
git commit -s -m "style(console): monospace only for code, ids and SQL (CF1 task 3)"
```

---

## CF1b — Primitives (D1077)

### How a T3 Code primitive comes over (Tasks 4–6)

Every vendored file goes through `scripts/studio/vendor.mjs` (SD1 Task 1). It copies the upstream bytes, writes the §53 §9.2 header naming `web/packages/ui/LICENSE.t3code`, and upserts the provenance row. Fetch the archive once, outside the repository:

```bash
node scripts/studio/vendor.mjs fetch --commit 33aa4614783e856977a5f0b856dc65ebc1ce6794 \
  --out ~/Documents/research/t3code-33aa4614-archive
```

Expected: `fetched https://github.com/pingdotgg/t3code/archive/33aa4614….zip`, its sha256, and the extracted entry count. The tool adds the archive to `web/packages/ui/provenance.json` on the first `import`.

Then import each file. The `--change` text is the fourth header line:

```bash
SRC=~/Documents/research/t3code-33aa4614-archive/t3code-33aa4614783e856977a5f0b856dc65ebc1ce6794
node scripts/studio/vendor.mjs import --package web/packages/ui --from "$SRC" --group k --verdict adapt \
  --change "ported to the design 59 tokens and cx, with Base UI underneath; no cva, lucide or Effect" \
  --map apps/web/src/components/ui/<name>.tsx=src/<name>.tsx
```

Then edit the copied file with this table. It is the same for every file:

| T3 Code | `@loams/ui` |
|---|---|
| `"use client";` | removed |
| `import { cn } from "~/lib/utils"` | `import { cx } from './cx'`; `cn(` → `cx(` |
| `cva(base, { variants })` and `VariantProps` | a `const VARIANT = { … } as const` record per axis, joined with `cx`; the prop types are `keyof typeof VARIANT` |
| `lucide-react` / `lucide` icons, `MorphIcon` | inline 16×16 SVGs written here (`chevron`, `check`, `x`, `minus`, `plus`: one `<path>` each, `stroke="currentColor"`, `strokeWidth={2}`, `aria-hidden`) |
| `~/components/ui/<x>` | `./<x>` |
| `bg-background` | `bg-bg` |
| `bg-popover`, `bg-card` | `bg-surface` |
| `text-foreground`, `text-popover-foreground`, `text-card-foreground` | `text-ink` |
| `text-muted-foreground`, `text-secondary-label`, `text-icon-muted` | `text-muted` |
| `placeholder:text-placeholder` | `placeholder:text-faint` |
| `bg-primary text-primary-foreground` | `bg-accent text-accent-ink` |
| `text-primary` | `text-accent-fg` |
| `bg-accent`, `bg-muted`, `bg-secondary` (hover and pressed fills) | `bg-raised` |
| `destructive` (any) | `danger` (`text-danger`, `bg-danger-soft`, `border-danger`) |
| `success`, `warning`, `info` | `grow`, `warn`, `info` |
| `border` with no colour | `border border-rule` |
| `border-input`, `dark:bg-input/32` | `border-input` (and drop the `dark:` fill) |
| `ring-*`, `focus-visible:ring-*`, `outline-ring` | `focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent` (§7) |
| `shadow-xs/5`, `inset-shadow-*`, `before:shadow-*` highlights | removed (elevation is hairlines) |
| popups `shadow-lg`, dialogs `shadow-xl` | `shadow-pop`, `shadow-modal` |
| `rounded-lg` on controls, `rounded-md` | `rounded-md` (8, controls) |
| `rounded-[.25rem]`, `rounded-sm` | `rounded-xs` (4) |
| popovers and menus `rounded-lg`/`rounded-xl` | `rounded-panel` (10) |
| dialogs `rounded-2xl` | `rounded-xl` (22) |
| `rounded-full` | `rounded-pill` |
| `h-9 sm:h-8` (default), `h-8 sm:h-7` (sm), `h-7` (compact) | `h-8` (32), `h-7` (28), `h-7` (28) (D1076) |
| input `h-8.5 sm:h-7.5` | `h-9` (36) |
| `text-base sm:text-sm`, `text-sm` | `text-body` (13) |
| `text-xs` | `text-small` (12) |
| `text-2xs`, `text-3xs` | `text-micro` (11) |
| `transition-[…] duration-200` on panels | `duration-(--op-panel-duration) ease-panel` |
| colour transitions | `duration-(--op-duration)` |
| `data-ending-style:scale-98` and translate animations | kept, with `motion-reduce:transition-none` |

Each file then joins `SPECIMENS` in `web/packages/ui/test/studio-primitives.test.tsx`. That scan fails on any T3 Code colour name, any Tailwind palette class and any corner outside the scale. This is the test that a primitive was "restyled onto the `--op-*` tokens".

When a T3 Code export's name is already a `@loams/ui` export with a different API, the T3 part takes a suffix and the Loams export keeps its name and props (Global Constraints):

| T3 Code export | Exported as | Why |
|---|---|---|
| `Input` | `InputControl` | `Input` is the labelled native input in `field.tsx` |
| `Textarea` | `TextareaControl` | same, `Textarea` |
| `Checkbox` | `CheckboxControl` | `Checkbox` is `field.tsx`'s labelled checkbox |
| `Select` (Root) | `SelectRoot` | `Select` is the native select; `SelectMenu` is the Loams wrapper |
| `Combobox` (Root) | `ComboboxRoot` | `Combobox` is the Loams wrapper |
| `Table` | `TableRoot` | `Table` is the Loams column table |
| `Group` / `ButtonGroup` | `ButtonGroup` | as upstream's alias |
| `Dialog`, `AlertDialog`, `Sheet`, `Popover`, `Menu`, `Tooltip` (Roots) | `DialogRoot`, `AlertDialogRoot`, `SheetRoot`, `PopoverRoot`, `MenuRoot`, `TooltipRoot` | the Loams wrappers keep the plain names (Task 5) |
| `ToastProvider`, `toastManager` | `ToastProvider` (same API), `toastManager` | Task 6 |

### Task 4: Controls and display primitives

**Files:**
- Modify (vendored over the Loams file): `web/packages/ui/src/button.tsx` ← `apps/web/src/components/ui/button.tsx`
- Create (vendored): `src/badge.tsx` ← `ui/badge.tsx`; `src/input.tsx` ← `ui/input.tsx`; `src/textarea.tsx` ← `ui/textarea.tsx`; `src/checkbox.tsx` ← `ui/checkbox.tsx`; `src/radio-group.tsx` ← `ui/radio-group.tsx`; `src/number-field.tsx` ← `ui/number-field.tsx`; `src/select.tsx` ← `ui/select.tsx`; `src/combobox.tsx` ← `ui/combobox.tsx`; `src/autocomplete.tsx` ← `ui/autocomplete.tsx`; `src/group.tsx` ← `ui/group.tsx`; `src/separator.tsx` ← `ui/separator.tsx`; `src/alert.tsx` ← `ui/alert.tsx`; `src/collapsible-section-header.tsx` ← `ui/collapsible-section-header.tsx`
- Modify (vendored parts added to a Loams file): `src/empty.tsx` ← `ui/empty.tsx`; `src/table.tsx` ← `ui/table.tsx`
- Modify: `src/status.tsx` (Badge moves out), `src/field.tsx` (unchanged API; `.loams-input` restyled in CSS), `src/styles.css` (`.loams-btn*`, `.loams-input`, `.loams-check`, `.loams-table*`, `.loams-empty*`; `.loams-badge` rules removed), `src/index.ts`
- Modify: `web/packages/ui/provenance.json` (archive `33aa4614` and 16 rows), `web/packages/ui/test/boundaries.test.ts:110` (verdicts)
- Modify: `web/packages/ui/src/command.tsx:1-4` provenance note only (autocomplete is now its own file)
- Create: `web/packages/ui/test/primitives-forms.test.tsx`
- Modify: `web/packages/ui/test/studio-primitives.test.tsx` (`SPECIMENS`)

**Interfaces:**
- Consumes: the tokens and utilities from Task 1.
- Produces (all from `@loams/ui`):
  - `Button` with `ButtonProps = useRender.ComponentProps<'button'> & { variant?: ButtonVariant; size?: ButtonSize }`, where `ButtonVariant = 'primary' | 'secondary' | 'quiet' | 'danger' | 'danger-solid' | 'link'` and `ButtonSize = 'xs' | 'sm' | 'md' | 'lg' | 'icon' | 'icon-sm'`; `buttonClass` unchanged for its old inputs.
  - `Badge` with `BadgeProps = useRender.ComponentProps<'span'> & { variant?: BadgeVariant; size?: 'sm' | 'md' | 'lg' }`, where `BadgeVariant = 'neutral' | 'accent' | 'success' | 'warning' | 'danger' | 'info' | 'outline'`.
  - `InputControl`, `TextareaControl`, `CheckboxControl`.
  - `RadioGroup`, `Radio`, `RadioGroupItem`.
  - `NumberField`, `NumberFieldGroup`, `NumberFieldDecrement`, `NumberFieldIncrement`, `NumberFieldInput`.
  - `SelectRoot`, `SelectTrigger`, `SelectValue`, `SelectPopup`, `SelectItem`, `SelectGroup`, `SelectGroupLabel`.
  - `ComboboxRoot`, `ComboboxInput`, `ComboboxTrigger`, `ComboboxPopup`, `ComboboxItem`, `ComboboxEmpty`, `ComboboxList`, `ComboboxClear`, `ComboboxStatus`, `useComboboxFilter`.
  - `Autocomplete`, `AutocompleteInput`, `AutocompleteTrigger`, `AutocompletePopup`, `AutocompleteItem`, `AutocompleteGroup`, `AutocompleteGroupLabel`, `AutocompleteEmpty`, `AutocompleteList`, `AutocompleteClear`, `AutocompleteCollection`.
  - `ButtonGroup`, `ButtonGroupSeparator`, `Separator`.
  - `Alert`, `AlertTitle`, `AlertDescription`, `AlertAction`.
  - `EmptyHeader`, `EmptyTitle`, `EmptyDescription`, `EmptyContent`, `EmptyMedia` (and `Empty` unchanged).
  - `TableRoot`, `TableHeader`, `TableBody`, `TableRow`, `TableHead`, `TableCell` (and `Table` unchanged).
  - `CollapsibleSectionHeader`, `SectionHeaderStatus`.

- [ ] **Step 1: Write the failing tests**

`web/packages/ui/test/primitives-forms.test.tsx`:

```tsx
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  ButtonGroup,
  buttonClass,
  CheckboxControl,
  CollapsibleSectionHeader,
  Empty,
  InputControl,
  NumberField,
  NumberFieldGroup,
  NumberFieldIncrement,
  NumberFieldInput,
  Radio,
  RadioGroup,
  SelectItem,
  SelectPopup,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  Separator,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRoot,
  TableRow,
} from '../src';

afterEach(cleanup);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.getAnimations ??= () => [];

describe('Button (vendored, Loams variants kept)', () => {
  it('is a type=button with the Loams class names and a data-slot', () => {
    render(<Button variant="primary">Save</Button>);
    const b = screen.getByRole('button', { name: 'Save' });
    expect(b.getAttribute('type')).toBe('button');
    expect(b.dataset.slot).toBe('button');
    expect(b.className).toContain('loams-btn loams-btn-primary');
  });

  it('renders as another element with render, and keeps buttonClass for old callers', () => {
    render(<Button render={<a href="#/x" />}>Open</Button>);
    const a = screen.getByRole('link', { name: 'Open' });
    expect(a.getAttribute('type')).toBeNull();
    expect(buttonClass({ size: 'sm' })).toBe('loams-btn loams-btn-secondary loams-btn-sm');
    expect(buttonClass()).toBe('loams-btn loams-btn-secondary');
  });
});

describe('Badge', () => {
  it('keeps the old call form and adds state variants', () => {
    render(
      <>
        <Badge title="edition">OSS</Badge>
        <Badge variant="danger">Failed</Badge>
      </>,
    );
    const oss = screen.getByText('OSS');
    expect(oss.dataset.slot).toBe('badge');
    expect(oss.getAttribute('title')).toBe('edition');
    expect(screen.getByText('Failed').className).toContain('bg-danger-soft');
  });
});

describe('form controls', () => {
  it('InputControl wraps an input and shows invalid state', () => {
    render(<InputControl aria-label="Name" aria-invalid />);
    const input = screen.getByRole('textbox', { name: 'Name' });
    expect(input.closest('[data-slot="input-control"]')?.className).toContain('has-aria-invalid:border-danger');
  });

  it('CheckboxControl toggles with a click', () => {
    render(<CheckboxControl aria-label="Remember" />);
    const box = screen.getByRole('checkbox', { name: 'Remember' });
    expect(box.getAttribute('aria-checked')).toBe('false');
    fireEvent.click(box);
    expect(box.getAttribute('aria-checked')).toBe('true');
  });

  it('RadioGroup moves with the arrow keys', () => {
    render(
      <RadioGroup aria-label="Size" defaultValue="s">
        <Radio value="s" aria-label="Small" />
        <Radio value="l" aria-label="Large" />
      </RadioGroup>,
    );
    const small = screen.getByRole('radio', { name: 'Small' });
    small.focus();
    fireEvent.keyDown(small, { key: 'ArrowDown' });
    expect(screen.getByRole('radio', { name: 'Large' }).getAttribute('aria-checked')).toBe('true');
  });

  it('NumberField increments', () => {
    render(
      <NumberField defaultValue={2}>
        <NumberFieldGroup>
          <NumberFieldInput aria-label="Replicas" />
          <NumberFieldIncrement aria-label="More" />
        </NumberFieldGroup>
      </NumberField>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'More' }));
    expect((screen.getByRole('textbox', { name: 'Replicas' }) as HTMLInputElement).value).toBe('3');
  });

  it('SelectRoot opens and picks an item', async () => {
    const picked: unknown[] = [];
    render(
      <SelectRoot defaultValue="a" onValueChange={(v) => picked.push(v)}>
        <SelectTrigger aria-label="Region">
          <SelectValue />
        </SelectTrigger>
        <SelectPopup>
          <SelectItem value="a">Amsterdam</SelectItem>
          <SelectItem value="b">Berlin</SelectItem>
        </SelectPopup>
      </SelectRoot>,
    );
    fireEvent.click(screen.getByRole('combobox', { name: 'Region' }));
    fireEvent.click(await screen.findByRole('option', { name: 'Berlin' }));
    await act(async () => {});
    expect(picked).toEqual(['b']);
  });
});

describe('display', () => {
  it('ButtonGroup, Separator and Alert carry their roles', () => {
    render(
      <>
        <ButtonGroup aria-label="Zoom">
          <Button>−</Button>
          <Button>+</Button>
        </ButtonGroup>
        <Separator />
        <Alert variant="warning">
          <AlertTitle>Branch is protected</AlertTitle>
          <AlertDescription>Writes need approval.</AlertDescription>
        </Alert>
      </>,
    );
    expect(screen.getByRole('group', { name: 'Zoom' })).toBeTruthy();
    expect(screen.getByRole('separator')).toBeTruthy();
    expect(screen.getByRole('alert').className).toContain('bg-warn-soft');
  });

  it('Empty keeps its props and no longer paints grain (Q929)', () => {
    const { container } = render(
      <Empty title="No branches" seed={3} actions={<Button>Create</Button>}>
        Branch from main to try a change.
      </Empty>,
    );
    expect(screen.getByText('No branches')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Create' })).toBeTruthy();
    expect(container.querySelector('canvas')).toBeNull();
  });

  it('TableRoot is a semantic table and CollapsibleSectionHeader reports its state', () => {
    render(
      <>
        <TableRoot aria-label="Tables">
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow>
              <TableCell>orders</TableCell>
            </TableRow>
          </TableBody>
        </TableRoot>
        <CollapsibleSectionHeader expanded={false}>Archived</CollapsibleSectionHeader>
      </>,
    );
    expect(screen.getByRole('table', { name: 'Tables' })).toBeTruthy();
    expect(screen.getByRole('button', { name: /Archived/ }).getAttribute('aria-expanded')).toBe('false');
  });
});
```

In `test/boundaries.test.ts`, change `expect(row.verdict).toBe('adapt');` to `expect(['as-is', 'adapt', 'rewrite']).toContain(row.verdict);` (W18). Task 6 adds the test that every design 59 primitive has its row, once the last row exists.

Add to `SPECIMENS` in `studio-primitives.test.tsx`: `['Button', () => <Button variant="primary">x</Button>]`, `['Badge', () => <Badge variant="info">x</Badge>]`, `['InputControl', () => <InputControl aria-label="x" />]`, `['CheckboxControl', () => <CheckboxControl aria-label="x" />]`, `['Alert', () => <Alert variant="danger"><AlertTitle>x</AlertTitle></Alert>]` and `['Separator', () => <Separator />]`.

- [ ] **Step 2: Run them and watch them fail**

Run: `pnpm --filter @loams/ui exec vitest run test/primitives-forms.test.tsx test/studio-primitives.test.tsx test/boundaries.test.ts`
Expected: FAIL, with `SyntaxError: The requested module '../src' does not provide an export named 'Alert'` (and the other new names).

- [ ] **Step 3: Vendor the files**

Run the `import` command from "How a T3 Code primitive comes over" with these maps in one call:

```
--map apps/web/src/components/ui/badge.tsx=src/badge.tsx
--map apps/web/src/components/ui/input.tsx=src/input.tsx
--map apps/web/src/components/ui/textarea.tsx=src/textarea.tsx
--map apps/web/src/components/ui/checkbox.tsx=src/checkbox.tsx
--map apps/web/src/components/ui/radio-group.tsx=src/radio-group.tsx
--map apps/web/src/components/ui/number-field.tsx=src/number-field.tsx
--map apps/web/src/components/ui/select.tsx=src/select.tsx
--map apps/web/src/components/ui/combobox.tsx=src/combobox.tsx
--map apps/web/src/components/ui/autocomplete.tsx=src/autocomplete.tsx
--map apps/web/src/components/ui/group.tsx=src/group.tsx
--map apps/web/src/components/ui/separator.tsx=src/separator.tsx
--map apps/web/src/components/ui/alert.tsx=src/alert.tsx
--map apps/web/src/components/ui/collapsible-section-header.tsx=src/collapsible-section-header.tsx
```

Copy `button.tsx`, `empty.tsx` and `table.tsx` into a scratch file each before the import, because the import overwrites them:

```
--map apps/web/src/components/ui/button.tsx=src/button.tsx
--map apps/web/src/components/ui/empty.tsx=src/empty.tsx
--map apps/web/src/components/ui/table.tsx=src/table.tsx
```

Then merge each one with the Loams exports, which keep their APIs (Steps 4–6). Expected tool output: one `imported <upstream> → <local> (adapt)` line per map. `node scripts/studio/vendor.mjs check --package web/packages/ui` must then print `ok`.

- [ ] **Step 4: Adapt `button.tsx`, `badge.tsx` and the inputs**

`src/button.tsx`, after the header the tool wrote. The `cva` table becomes the Loams class names, which `styles.css` paints, so `buttonClass` keeps working outside React:

```tsx
import { mergeProps } from '@base-ui/react/merge-props';
import { useRender } from '@base-ui/react/use-render';
import { cx } from './cx';

export type ButtonVariant = 'primary' | 'secondary' | 'quiet' | 'danger' | 'danger-solid' | 'link';
export type ButtonSize = 'xs' | 'sm' | 'md' | 'lg' | 'icon' | 'icon-sm';

/** The class list for a button, for links styled as buttons and callers outside React. */
export function buttonClass({
  variant = 'secondary',
  size = 'md',
  className,
}: { variant?: ButtonVariant; size?: ButtonSize; className?: string } = {}): string {
  return cx('loams-btn', `loams-btn-${variant}`, size !== 'md' && `loams-btn-${size}`, className);
}

export type ButtonProps = useRender.ComponentProps<'button'> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
};

/** The one action control. `type` defaults to "button"; `render` swaps the element. */
export function Button({ variant, size, className, render, ...props }: ButtonProps) {
  const defaults = {
    className: buttonClass({ variant, size, className }),
    'data-slot': 'button',
    'data-variant': variant ?? 'secondary',
    type: render ? undefined : ('button' as const),
  };
  return useRender({ defaultTagName: 'button', props: mergeProps<'button'>(defaults, props), render });
}
```

Delete upstream's `InlineButton`, which `@loams/ui` does not need. Replace the `.loams-btn` block in `styles.css` (lines 30–114) with:

```css
.loams-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: var(--op-control);
  padding: 0 11px;
  border: 1px solid transparent;
  border-radius: var(--op-radius-md);
  font: inherit;
  font-size: var(--op-text-body);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  color: var(--op-ink);
  background: none;
  cursor: pointer;
  text-decoration: none;
  transition:
    background-color var(--op-duration) ease,
    border-color var(--op-duration) ease,
    color var(--op-duration) ease;
}
.loams-btn:focus-visible {
  outline: 2px solid var(--op-accent);
  outline-offset: 2px;
}
.loams-btn:disabled,
.loams-btn[aria-disabled="true"] {
  opacity: 0.55;
  cursor: not-allowed;
}
.loams-btn-primary {
  color: var(--op-accent-ink);
  background: var(--op-accent);
  border-color: var(--op-accent);
}
.loams-btn-primary:hover:not(:disabled) {
  background: color-mix(in oklab, var(--op-accent) 90%, var(--op-bg));
}
.loams-btn-secondary {
  background: var(--op-surface);
  border-color: var(--op-input);
}
.loams-btn-secondary:hover:not(:disabled) {
  background: var(--op-raised);
}
.loams-btn-quiet {
  color: var(--op-muted);
}
.loams-btn-quiet:hover:not(:disabled) {
  color: var(--op-ink);
  background: var(--op-raised);
}
.loams-btn-danger {
  color: var(--op-danger);
  background: var(--op-surface);
  border-color: var(--op-input);
}
.loams-btn-danger:hover:not(:disabled) {
  background: var(--op-danger-soft);
  border-color: color-mix(in oklab, var(--op-danger) 32%, transparent);
}
.loams-btn-danger-solid {
  color: var(--op-bg);
  background: var(--op-danger);
  border-color: var(--op-danger);
}
.loams-btn-link {
  height: auto;
  padding: 0;
  color: var(--op-accent-fg);
  text-decoration: underline;
  text-decoration-color: transparent;
  text-underline-offset: 3px;
}
.loams-btn-link:hover:not(:disabled) {
  text-decoration-color: currentColor;
}
.loams-btn-xs {
  height: 24px;
  gap: 4px;
  padding: 0 7px;
  font-size: var(--op-text-small);
  border-radius: var(--op-radius-sm);
}
.loams-btn-sm {
  height: var(--op-control-compact);
  padding: 0 9px;
  font-size: var(--op-text-small);
}
.loams-btn-lg {
  height: var(--op-control-input);
  padding: 0 13px;
  font-size: var(--op-text-emphasis);
}
.loams-btn-icon {
  width: var(--op-control);
  padding: 0;
}
.loams-btn-icon-sm {
  width: var(--op-control-compact);
  height: var(--op-control-compact);
  padding: 0;
}
```

`src/badge.tsx` (`cva` becomes records; the default is `neutral`, the old Badge's look):

```tsx
import { mergeProps } from '@base-ui/react/merge-props';
import { useRender } from '@base-ui/react/use-render';
import { cx } from './cx';

const VARIANT = {
  neutral: 'bg-raised text-muted',
  accent: 'bg-accent-soft text-accent-fg',
  success: 'bg-grow-soft text-grow',
  warning: 'bg-warn-soft text-warn',
  danger: 'bg-danger-soft text-danger',
  info: 'bg-info-soft text-info',
  outline: 'border-rule bg-transparent text-ink',
} as const;
const SIZE = {
  sm: 'h-4 min-w-4 px-1 text-micro leading-none',
  md: 'h-5 min-w-5 px-1.5 text-micro',
  lg: 'h-6 min-w-6 px-2 text-small',
} as const;

export type BadgeVariant = keyof typeof VARIANT;
export type BadgeProps = useRender.ComponentProps<'span'> & {
  variant?: BadgeVariant;
  size?: keyof typeof SIZE;
};

export function Badge({ className, variant = 'neutral', size = 'md', render, ...props }: BadgeProps) {
  const defaults = {
    className: cx(
      'loams-badge relative inline-flex shrink-0 items-center justify-center gap-1 whitespace-nowrap',
      'rounded-xs border border-transparent font-medium [&_svg]:size-3 [&_svg]:shrink-0',
      VARIANT[variant],
      SIZE[size],
      className,
    ),
    'data-slot': 'badge',
  };
  return useRender({ defaultTagName: 'span', props: mergeProps<'span'>(defaults, props), render });
}
```

Delete the `.loams-badge { … }` rule from `styles.css`. Unlayered CSS beats Tailwind's `utilities` layer, so the old rule would override the variants. `status.tsx` drops its `Badge`, and `index.ts` exports `Badge`, `BadgeProps` and `BadgeVariant` from `./badge`.

`src/input.tsx` (only the class strings and the export name change; `nativeInput` and the `type` adjustments stay as upstream):

```tsx
export type InputControlProps = Omit<InputPrimitive.Props & React.RefAttributes<HTMLInputElement>, 'size'> & {
  size?: 'compact' | 'default' | 'lg' | number;
  font?: 'default' | 'mono';
  unstyled?: boolean;
  nativeInput?: boolean;
};

export function InputControl({ className, size = 'default', font = 'default', unstyled = false, nativeInput = false, ...props }: InputControlProps) {
  const inputClassName = cx(
    'h-9 w-full min-w-0 rounded-[inherit] bg-transparent px-3 leading-9 outline-none placeholder:text-faint',
    size === 'compact' && 'h-7 px-2.5 text-small leading-7',
    size === 'lg' && 'h-10 leading-10',
    props.type === 'search' &&
      '[&::-webkit-search-cancel-button]:appearance-none [&::-webkit-search-decoration]:appearance-none',
    props.type === 'number' &&
      '[appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none',
    props.type === 'file' && 'text-muted file:me-3 file:bg-transparent file:font-medium file:text-ink file:text-body',
  );
  // Upstream lines 35-60 (`let inputElement` and the `nativeInput` branch) are kept verbatim.
  return (
    <span
      className={cx(
        !unstyled &&
          'relative inline-flex w-full rounded-md border border-input bg-bg text-body text-ink ' +
            'has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-accent ' +
            'has-aria-invalid:border-danger has-disabled:opacity-60',
        !unstyled && size === 'compact' && 'rounded-sm',
        font === 'mono' && 'font-mono tabular-nums',
        className,
      )}
      data-size={size}
      data-slot="input-control"
    >
      {inputElement}
    </span>
  );
}
```

`src/textarea.tsx` gets the same wrapper classes as `TextareaControl`. The inner textarea is `'field-sizing-content min-h-18 max-h-64 w-full rounded-[inherit] bg-transparent px-3 py-2 outline-none wrap-anywhere'`. `src/checkbox.tsx` becomes `CheckboxControl`; its `Root` and `Indicator` class lists change as follows, and the rest of upstream's file (the `render` with its two SVGs) stays:

```tsx
      className={cx(
        'relative inline-flex size-4 shrink-0 items-center justify-center rounded-xs border border-input bg-bg outline-none',
        'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
        'aria-invalid:border-danger data-disabled:opacity-60',
        className,
      )}
      data-slot="checkbox"
      {...props}
    >
      <CheckboxPrimitive.Indicator
        className="absolute -inset-px flex items-center justify-center rounded-xs text-accent-ink data-unchecked:hidden data-checked:bg-accent data-indeterminate:text-ink"
```

Keep upstream's two check and minus SVGs. `src/radio-group.tsx` takes the same classes with `rounded-pill`, and its dot is `before:bg-accent-ink`.

In `styles.css`, restyle the native controls so `field.tsx` matches. `.loams-input` gets `height: var(--op-control-input); border: 1px solid var(--op-input); border-radius: var(--op-radius-md); background: var(--op-bg); font-size: var(--op-text-body); padding: 0 11px;`. `.loams-input:focus-visible` gets `outline: 2px solid var(--op-accent); outline-offset: 2px;`. `textarea.loams-input` gets `height: auto; padding: 8px 11px;`.

- [ ] **Step 5: Adapt the pickers**

`src/select.tsx`: drop `cva` and lucide, and export the root as `SelectRoot`. The trigger and item are the key adaptations:

```tsx
export const SelectRoot = SelectPrimitive.Root;

const TRIGGER_SIZE = { compact: 'h-7 gap-1 px-2 text-small', default: 'h-8 px-3', lg: 'h-9 px-3' } as const;
const TRIGGER_VARIANT = {
  default: 'w-full min-w-36 border-input bg-bg text-ink',
  ghost: 'border-transparent text-muted hover:bg-raised hover:text-ink data-pressed:bg-raised',
} as const;

export function SelectTrigger({ className, size = 'default', variant = 'default', children, icon, ...props }:
  SelectPrimitive.Trigger.Props & { size?: keyof typeof TRIGGER_SIZE; variant?: keyof typeof TRIGGER_VARIANT; icon?: ReactNode }) {
  return (
    <SelectPrimitive.Trigger
      className={cx(
        'relative inline-flex cursor-pointer select-none items-center justify-between gap-2 rounded-md border text-left text-body outline-none',
        'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent aria-invalid:border-danger data-disabled:opacity-60',
        TRIGGER_VARIANT[variant],
        TRIGGER_SIZE[size],
        className,
      )}
      data-slot="select-trigger"
      {...props}
    >
      {children}
      <SelectPrimitive.Icon data-slot="select-icon">{icon ?? <ChevronDown className="-me-1 size-3 text-muted" />}</SelectPrimitive.Icon>
    </SelectPrimitive.Trigger>
  );
}
```

`ChevronDown` is a local component in `select.tsx`:

```tsx
const ChevronDown = ({ className }: { className?: string }) => (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} aria-hidden="true" className={className}>
    <path d="M4 6l4 4 4-4" />
  </svg>
);
```

`SelectPopup` uses `rounded-panel border border-rule bg-surface p-1 text-ink shadow-pop`. `SelectItem` uses `flex min-h-7 cursor-default items-center gap-2 rounded-sm px-2 text-body outline-none data-highlighted:bg-raised data-disabled:opacity-50`. Upstream's scroll arrows keep their behaviour, with inline chevrons.

`src/combobox.tsx` and `src/autocomplete.tsx` follow the table. Their popups use `SelectPopup`'s classes, and their inputs reuse `InputControl` (upstream imports `~/components/ui/input`, so rewrite it to `./input` and use `InputControl`). Upstream's `ComboboxListVirtualized` and `AutocompleteListVirtualized` depend on `@legendapp/list`. Drop them, and add the note "the virtualised lists are dropped; long lists use VirtualList" to both rows in `provenance.json`. `src/number-field.tsx` swaps lucide's `MinusIcon`/`PlusIcon` for local SVGs (`M3 8h10` and `M8 3v10M3 8h10`), and its group uses `InputControl`'s wrapper classes.

Point the provenance note of `command.tsx` at the new file: "merges apps/web/src/components/ui/dialog-styles.ts; autocomplete's popup parts are src/autocomplete.tsx since CF1; @legendapp/list replaced by TanStack Virtual". Change only the note; `command.tsx` keeps its commit `6524a8d4`.

- [ ] **Step 6: Adapt the display parts and merge `empty.tsx` and `table.tsx`**

`src/group.tsx` exports `ButtonGroup` (`role="group"`, `inline-flex [&>[data-slot=button]:not(:first-child)]:rounded-s-none [&>[data-slot=button]:not(:last-child)]:rounded-e-none`) and `ButtonGroupSeparator` (`Separator` with `orientation="vertical"`). `src/separator.tsx` changes only `bg-border` → `bg-rule`. `src/alert.tsx`:

```tsx
const VARIANT = {
  default: 'border-rule bg-surface text-ink',
  info: 'border-transparent bg-info-soft text-ink [&_[data-slot=alert-title]]:text-info',
  success: 'border-transparent bg-grow-soft text-ink [&_[data-slot=alert-title]]:text-grow',
  warning: 'border-transparent bg-warn-soft text-ink [&_[data-slot=alert-title]]:text-warn',
  danger: 'border-transparent bg-danger-soft text-ink [&_[data-slot=alert-title]]:text-danger',
} as const;

export function Alert({ className, variant = 'default', ...props }: ComponentProps<'div'> & { variant?: keyof typeof VARIANT }) {
  return (
    <div
      role="alert"
      data-slot="alert"
      className={cx('relative grid w-full gap-1 rounded-lg border px-3.5 py-3 text-body', VARIANT[variant], className)}
      {...props}
    />
  );
}
```

Upstream's `error` variant is `danger` here (the table), and `AlertTitle`, `AlertDescription` and `AlertAction` keep upstream's layout classes. `src/collapsible-section-header.tsx` keeps upstream's markup with these tones:

```ts
const tones = {
  muted: { label: 'text-faint', line: 'bg-rule' },
  info: { label: 'text-info', line: 'bg-info-soft' },
  emphasized: { label: 'text-muted', line: 'bg-rule' },
  accent: { label: 'text-accent-fg', line: 'bg-accent-soft' },
} as const;
```

It also gets `text-small font-medium`, `rounded-md` and the §7 focus outline. The lucide `ChevronDownIcon` becomes the local `ChevronDown` SVG, and `SectionHeaderStatus` uses `text-micro text-danger`.

`src/empty.tsx` (the vendored parts, then the Loams `Empty` built from them; `grain` and `seed` are still accepted and ignored, Q929):

```tsx
export function EmptyHeader({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-header" className={cx('flex max-w-sm flex-col items-center gap-2 text-center', className)} {...props} />;
}
export function EmptyTitle({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-title" className={cx('text-emphasis font-semibold text-ink', className)} {...props} />;
}
export function EmptyDescription({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-description" className={cx('text-body text-muted [&_a]:text-accent-fg', className)} {...props} />;
}
export function EmptyContent({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-content" className={cx('flex w-full max-w-sm flex-col items-center gap-3 text-body', className)} {...props} />;
}
export function EmptyMedia({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-icon" className={cx('mb-2 grid size-9 place-items-center rounded-md bg-raised text-muted [&_svg]:size-4.5', className)} {...props} />;
}

/** An empty state says what goes here and how to add it. `grain` and `seed` are kept for callers and unused (Q929). */
export function Empty({ title, children, actions }: {
  title: string; children?: ReactNode; actions?: ReactNode; grain?: string; seed?: number;
}) {
  return (
    <div className="loams-empty" data-slot="empty">
      <EmptyHeader>
        <EmptyTitle>{title}</EmptyTitle>
        {children && <EmptyDescription className="loams-empty-body">{children}</EmptyDescription>}
      </EmptyHeader>
      {actions && <EmptyContent className="loams-empty-actions">{actions}</EmptyContent>}
    </div>
  );
}
```

In `styles.css`, delete `.loams-empty .loams-grain` and `.loams-empty > :not(.loams-grain)`. `.loams-empty` becomes `display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 40px 24px; border: 1px dashed var(--op-rule); border-radius: var(--op-radius-lg);`.

`src/table.tsx`: upstream's six parts come first, exported as `TableRoot`, `TableHeader`, `TableBody`, `TableRow`, `TableHead` and `TableCell`. Their classes are `text-small`, `border-rule`, `hover:bg-row-hover`, `data-[state=selected]:bg-row-selected` and `h-9 px-2 text-left font-medium text-muted`. The Loams `Table`, `Column` and `Primary` follow, unchanged. Restyle `.loams-table th` to `font-size: var(--op-text-small); font-weight: 500; color: var(--op-muted);` and `.loams-table td` to `font-size: var(--op-text-body); padding: 8px 10px;`. Data cells stay 12–13px (D1075).

Export everything listed under "Produces" from `src/index.ts`.

- [ ] **Step 7: Run the tests**

```bash
pnpm --filter @loams/ui test
node scripts/studio/vendor.mjs check --package web/packages/ui
pnpm -C web -r --if-present test
pnpm -C web typecheck
```

Expected: PASS, and `ok` from `check`.

- [ ] **Step 8: Commit**

```bash
git add web/packages/ui
git commit -s -m "feat(ui): vendor T3 Code's controls and display primitives onto the tokens (CF1 task 4)"
```

---

### Task 5: Overlays: dialog, alert dialog, sheet, popover, preview card, menu, tooltip

**Files:**
- Create (vendored): `web/packages/ui/src/dialog-styles.ts` ← `ui/dialog-styles.ts`; `src/dialog.tsx` ← `ui/dialog.tsx`; `src/alert-dialog.tsx` ← `ui/alert-dialog.tsx`; `src/sheet.tsx` ← `ui/sheet.tsx`; `src/popover.tsx` ← `ui/popover.tsx`; `src/preview-card.tsx` ← `ui/preview-card.tsx`; `src/menu.tsx` ← `ui/menu.tsx`; `src/tooltip.tsx` ← `ui/tooltip.tsx`
- Create: `src/tabs.tsx` (the Loams `Tabs` and `Tab`, moved unchanged from `overlay.tsx:423-473`; not vendored)
- Modify: `src/select.tsx`, `src/combobox.tsx` (take in `SelectMenu`, `Option` and `Combobox` from `overlay.tsx:304-422`, rebuilt on Task 4's parts)
- Delete: `src/overlay.tsx` (after its wrappers move: `Dialog`→`dialog.tsx`, `AlertDialog`→`alert-dialog.tsx`, `Sheet`→`sheet.tsx`, `Menu`/`MenuItem`/`MenuLabel`/`MenuSeparator`→`menu.tsx`, `Popover`→`popover.tsx`, `Tooltip`/`TooltipProvider`→`tooltip.tsx`; `Toaster`/`ToastProvider`/`useToast` move in Task 6, so in this task they go temporarily to `src/toast.tsx` unchanged)
- Modify: `src/styles.css:621-700` (the dialog block becomes the glass classes), `src/index.ts`, `provenance.json` (8 rows)
- Create: `web/packages/ui/test/primitives-overlays.test.tsx`
- Modify: `web/packages/ui/test/studio-primitives.test.tsx` (`SPECIMENS`)

**Interfaces:**
- Consumes: `Button` (Task 4), `ScrollArea`, the tokens `--op-glass`, `--op-blur`, `--op-dialog-highlight`, `--op-scrim`.
- Produces:
  - The unchanged wrappers: `Dialog`, `AlertDialog`, `Sheet`, `Menu`, `MenuItem`, `MenuLabel`, `MenuSeparator`, `Popover`, `SelectMenu`, `Option`, `Combobox`, `Tabs`, `Tab`, `Tooltip`, `TooltipProvider`.
  - The parts: `DIALOG_BACKDROP_CLASS`, `DIALOG_POPUP_CLASS`; `DialogRoot`, `DialogTrigger`, `DialogPortal`, `DialogClose`, `DialogBackdrop`, `DialogViewport`, `DialogPopup`, `DialogHeader`, `DialogFooter`, `DialogTitle`, `DialogDescription`, `DialogPanel`; `AlertDialogRoot`, `AlertDialogPopup`, `AlertDialogHeader`, `AlertDialogFooter`, `AlertDialogTitle`, `AlertDialogDescription`, `AlertDialogClose`; `SheetRoot`, `SheetPopup`, `SheetHeader`, `SheetTitle`, `SheetDescription`; `PopoverRoot`, `PopoverTrigger`, `PopoverPopup`, `PopoverClose`, `PopoverTitle`; `PreviewCard`, `PreviewCardTrigger`, `PreviewCardPopup`; `MenuRoot`, `MenuTrigger`, `MenuPopup`, `MenuGroup`, `MenuGroupLabel`, `MenuCheckboxItem`, `MenuRadioGroup`, `MenuRadioItem`, `MenuShortcut`, `MenuSub`, `MenuSubTrigger`, `MenuSubPopup`; `TooltipRoot`, `TooltipTrigger`, `TooltipPopup`.
  - Upstream `MenuItem` and `MenuSeparator` are exported as `MenuPart` and `MenuPartSeparator`, because the Loams wrappers keep those names.

- [ ] **Step 1: Write the failing tests**

The existing overlay tests (`test/primitives.test.tsx` "Dialog", "Menu", "Tabs"; `test/studio-primitives.test.tsx` "Menu", "AlertDialog", "Sheet") must pass unchanged. They are the API-compatibility proof. Add `web/packages/ui/test/primitives-overlays.test.tsx`:

```tsx
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import {
  DIALOG_POPUP_CLASS,
  Dialog,
  MenuPopup,
  MenuRadioGroup,
  MenuRadioItem,
  MenuRoot,
  MenuTrigger,
  PopoverPopup,
  PopoverRoot,
  PopoverTrigger,
  PreviewCard,
  PreviewCardPopup,
  PreviewCardTrigger,
  SelectMenu,
  TooltipPopup,
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
} from '../src';

afterEach(cleanup);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.getAnimations ??= () => [];

describe('dialog glass (D1073)', () => {
  it('a dialog popup is glass with the xl radius and the no-drag region', async () => {
    render(
      <Dialog open onClose={() => {}} title="Rename">
        body
      </Dialog>,
    );
    await act(async () => {});
    const dialog = screen.getByRole('dialog', { name: 'Rename' });
    expect(dialog.className).toContain('loams-dialog-glass');
    expect(DIALOG_POPUP_CLASS).toContain('[-webkit-app-region:no-drag]');
  });

  it('Escape closes the topmost overlay only (D1081)', async () => {
    const closed: string[] = [];
    render(
      <>
        <Dialog open onClose={() => closed.push('outer')} title="Outer">
          <Dialog open onClose={() => closed.push('inner')} title="Inner">
            inner
          </Dialog>
        </Dialog>
      </>,
    );
    await act(async () => {});
    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
    expect(closed).toEqual(['inner']);
  });
});

describe('vendored parts', () => {
  it('a radio menu reports the chosen item', async () => {
    const chosen: string[] = [];
    render(
      <MenuRoot>
        <MenuTrigger>Sort</MenuTrigger>
        <MenuPopup>
          <MenuRadioGroup value="name" onValueChange={(v) => chosen.push(String(v))}>
            <MenuRadioItem value="name">Name</MenuRadioItem>
            <MenuRadioItem value="date">Date</MenuRadioItem>
          </MenuRadioGroup>
        </MenuPopup>
      </MenuRoot>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Sort' }));
    fireEvent.click(await screen.findByRole('menuitemradio', { name: 'Date' }));
    expect(chosen).toEqual(['date']);
  });

  it('a popover opens from its trigger, and a tooltip names its trigger', async () => {
    render(
      <TooltipProvider>
        <PopoverRoot>
          <PopoverTrigger>Filters</PopoverTrigger>
          <PopoverPopup>filter body</PopoverPopup>
        </PopoverRoot>
        <TooltipRoot open>
          <TooltipTrigger render={<button type="button">i</button>} />
          <TooltipPopup>Inspector (⌘I)</TooltipPopup>
        </TooltipRoot>
        <PreviewCard open>
          <PreviewCardTrigger href="#/data">orders</PreviewCardTrigger>
          <PreviewCardPopup>12 columns</PreviewCardPopup>
        </PreviewCard>
      </TooltipProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Filters' }));
    expect(await screen.findByText('filter body')).toBeTruthy();
    expect(await screen.findByText('Inspector (⌘I)')).toBeTruthy();
    expect(await screen.findByText('12 columns')).toBeTruthy();
  });

  it('SelectMenu keeps its props on the new parts', async () => {
    const picked: string[] = [];
    render(
      <SelectMenu
        label="Region"
        value="ams"
        onValueChange={(v) => picked.push(v)}
        options={[
          { value: 'ams', label: 'Amsterdam' },
          { value: 'ber', label: 'Berlin' },
        ]}
      />,
    );
    fireEvent.click(screen.getByRole('combobox', { name: 'Region' }));
    fireEvent.click(await screen.findByRole('option', { name: 'Berlin' }));
    expect(picked).toEqual(['ber']);
  });
});
```

Add to `SPECIMENS`: `['Dialog', () => <Dialog open onClose={() => {}} title="t">b</Dialog>]`, the popover and menu above (opened), and `['TooltipPopup', …]`.

- [ ] **Step 2: Run them and watch them fail**

Run: `pnpm --filter @loams/ui exec vitest run test/primitives-overlays.test.tsx`
Expected: FAIL, with `does not provide an export named 'DIALOG_POPUP_CLASS'`.

- [ ] **Step 3: Vendor and adapt**

Import the eight files with `vendor.mjs import`, using the maps from "Files". Then apply the class table. The key adaptations follow.

`src/dialog-styles.ts` (glass is a Loams CSS class; the `[-webkit-app-region:no-drag]` keeps dialogs clickable over the desktop's drag region):

```ts
const DIALOG_BACKDROP_BASE_CLASS =
  'fixed inset-0 z-50 transition-opacity duration-(--op-duration) data-ending-style:opacity-0 data-starting-style:opacity-0 motion-reduce:transition-none';

export const DIALOG_BACKDROP_CLASS = `loams-dialog-scrim loams-dialog-backdrop ${DIALOG_BACKDROP_BASE_CLASS}`;

const DIALOG_POPUP_BASE_CLASS =
  '-translate-y-[calc(1.25rem*var(--nested-dialogs))] relative flex min-h-0 w-full min-w-0 scale-[calc(1-0.1*var(--nested-dialogs))] flex-col ' +
  'opacity-[calc(1-0.1*var(--nested-dialogs))] text-ink outline-none transition-[scale,opacity,translate] duration-(--op-duration) ' +
  'data-ending-style:scale-98 data-starting-style:scale-98 data-ending-style:opacity-0 data-starting-style:opacity-0 ' +
  'motion-reduce:transition-none [-webkit-app-region:no-drag]';

export const DIALOG_POPUP_CLASS = `loams-dialog loams-dialog-glass ${DIALOG_POPUP_BASE_CLASS} rounded-xl`;

export const DIALOG_MOBILE_SHEET_CLASS =
  'max-sm:max-w-none max-sm:rounded-none max-sm:border-x-0 max-sm:border-b-0 max-sm:data-ending-style:translate-y-4 max-sm:data-starting-style:translate-y-4';
```

Drop upstream's `DIALOG_MEDIA_*` classes; `@loams/ui` has no media lightbox. In `styles.css`, replace the dialog block (lines 621–700) with:

```css
/* Dialog glass (D1073): 80% opacity over a 14px blur; dark adds a 4% white inset. */
.loams-dialog-glass {
  background: color-mix(in srgb, var(--op-surface) var(--op-glass), transparent);
  -webkit-backdrop-filter: blur(var(--op-blur)) saturate(1.1);
  backdrop-filter: blur(var(--op-blur)) saturate(1.1);
  border: 1px solid var(--op-rule);
  box-shadow: var(--op-dialog-highlight), var(--op-shadow-modal);
}
.loams-dialog-backdrop {
  background: var(--op-scrim);
  -webkit-backdrop-filter: blur(4px);
  backdrop-filter: blur(4px);
}
.loams-dialog-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 16px 20px 8px;
}
.loams-dialog-head h2 {
  margin: 0;
  font-size: var(--op-text-section);
  font-weight: 600;
}
.loams-dialog-body {
  padding: 8px 20px 16px;
  font-size: var(--op-text-body);
}
.loams-dialog-foot {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 20px;
  border-top: 1px solid var(--op-rule-soft);
}
```

`src/dialog.tsx`: upstream's parts, with the lucide `XIcon` close replaced by `×`, and `DialogRoot = DialogPrimitive.Root`. Then the Loams wrapper, with the same props as before:

```tsx
export function Dialog({ open, onClose, title, children, footer, dismissible = true }: {
  open: boolean; onClose: () => void; title: ReactNode; children: ReactNode; footer?: ReactNode; dismissible?: boolean;
}) {
  return (
    <DialogRoot
      open={open}
      onOpenChange={(next) => {
        if (!next && dismissible) onClose();
      }}
      disablePointerDismissal={!dismissible}
    >
      <DialogPopup showCloseButton={false} className="max-w-lg">
        <div className="loams-dialog-head">
          <DialogTitle>{title}</DialogTitle>
          <DialogClose render={<Button variant="quiet" size="icon-sm" />} aria-label="Close" disabled={!dismissible}>
            ×
          </DialogClose>
        </div>
        <div className="loams-dialog-body">{children}</div>
        {footer && <div className="loams-dialog-foot">{footer}</div>}
      </DialogPopup>
    </DialogRoot>
  );
}
```

`DialogPopup` portals itself, with `DialogBackdrop` and `DialogViewport`, as upstream does. Keep upstream's `showCloseButton` prop. `src/alert-dialog.tsx` puts upstream's parts (`AlertDialogRoot = AlertDialogPrimitive.Root`) under the Loams `AlertDialog`, whose props (`open`, `onClose`, `title`, `children`, `confirm`, `cancel`, `onConfirm`, `tone`, `pending`) and behaviour (no scrim dismissal, held while `pending`) are unchanged:

```tsx
    <AlertDialogRoot open={open} onOpenChange={(next) => { if (!next && !pending) onClose(); }}>
      <AlertDialogPopup>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          {children && <AlertDialogDescription>{children}</AlertDialogDescription>}
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogClose render={<Button variant="quiet" />} disabled={pending}>{cancel}</AlertDialogClose>
          <Button variant={tone === 'danger' ? 'danger-solid' : 'primary'} onClick={onConfirm} disabled={pending} aria-busy={pending || undefined}>
            {confirm}
          </Button>
        </AlertDialogFooter>
      </AlertDialogPopup>
    </AlertDialogRoot>
```

`AlertDialogFooter`'s `default` variant uses `border-t border-rule-soft bg-raised py-3`. The title uses `text-section font-semibold`, and the description uses `text-body text-muted`. `src/sheet.tsx` keeps upstream's parts and the Loams `Sheet({ open, onClose, title, side, children })`. Its sides are the old `SHEET_SIDE` table. Its panel uses `bg-surface border-rule shadow-modal`, and the slide transition is `duration-(--op-panel-duration) ease-panel motion-reduce:transition-none`, so it snaps unless motion is on (D1076). `src/menu.tsx` exports upstream's parts with `MenuItem` renamed `MenuPart` and `MenuSeparator` renamed `MenuPartSeparator`, then the Loams `Menu`, `MenuItem`, `MenuLabel` and `MenuSeparator` rebuilt on them, with their props unchanged. The popup classes are `rounded-panel border border-rule bg-surface p-1 shadow-pop text-body`. Items are `flex min-h-7 items-center gap-2 rounded-sm px-2 data-highlighted:bg-raised`. `MenuShortcut` uses `ms-auto text-small text-faint`, with no mono (D1075). `src/popover.tsx` and `src/preview-card.tsx` use the same popup classes. `src/tooltip.tsx` keeps upstream's hover and scroll-dismiss contexts. Its popup is `rounded-sm bg-ink px-2 py-1 text-small text-bg shadow-pop`, and upstream's `~/lib/windowControlsOverlay` import is dropped (its collision padding becomes a constant 5). The Loams `Tooltip({ content, children })` becomes:

```tsx
export function Tooltip({ content, children }: { content: ReactNode; children: ReactElement }) {
  return (
    <TooltipRoot>
      <TooltipTrigger render={children} />
      <TooltipPopup>{content}</TooltipPopup>
    </TooltipRoot>
  );
}
```

`SelectMenu` (in `select.tsx`) and `Combobox` (in `combobox.tsx`) are rebuilt on `SelectRoot`/`SelectTrigger`/`SelectPopup`/`SelectItem` and `ComboboxRoot`/`ComboboxInput`/`ComboboxPopup`/`ComboboxItem`. Their props (`label`, `value`, `onValueChange`, `options`, `className`, `leading`, `icon`, and the combobox's) are unchanged. Move `Tabs`/`Tab` byte for byte into `src/tabs.tsx`. Move `Toaster`, `ToastProvider` and `useToast` into `src/toast.tsx` unchanged; Task 6 replaces them. Delete `overlay.tsx`, and point `index.ts` at the new files.

- [ ] **Step 4: Run the tests**

```bash
pnpm --filter @loams/ui test
node scripts/studio/vendor.mjs check --package web/packages/ui
pnpm -C web -r --if-present test
pnpm -C web typecheck
```

Expected: PASS, including every existing overlay test.

- [ ] **Step 5: Commit**

```bash
git add web/packages/ui
git commit -s -m "feat(ui): vendor T3 Code's overlays; the Loams wrappers keep their props (CF1 task 5)"
```

---

### Task 6: Toasts, destructive confirmations and typed confirmation

**Files:**
- Modify (vendored over the Task 5 move): `web/packages/ui/src/toast.tsx` ← `ui/toast.tsx`
- Create (vendored): `src/toast-helpers.ts` ← `ui/toastHelpers.ts`; `src/sidebar-state.ts` ← `ui/sidebarState.ts` (verdict `as-is`)
- Create: `src/confirm.tsx` (Loams: `ConfirmDialog`, `TypedConfirmDialog`)
- Modify: `src/index.ts`, `provenance.json` (3 rows), `test/boundaries.test.ts` (the design 59 row list)
- Create: `web/packages/ui/test/toast-confirm.test.tsx`
- Modify: `web/plugins/shell/src/layout.tsx` (no change in the call: `<ToastProvider>` and `<Toaster />` keep their names)

**Interfaces:**
- Consumes: `AlertDialog*` parts and `Button` (Tasks 4–5), `InputControl`.
- Produces:
  - `ToastProvider` (same name).
  - `useToast(): { add(o: ToastOptions): string; close(id: string): void; update(id: string, o: Partial<ToastOptions>): void }`.
  - `ToastOptions = { id?: string; title: ReactNode; description?: ReactNode; type?: 'success' | 'error' | 'warning' | 'info' | 'loading' | 'danger'; timeout?: number; priority?: 'low' | 'high'; actionProps?: ComponentProps<'button'>; data?: ToastData }`, where `'danger'` is an alias of `'error'`.
  - `ToastData = { leadingIcon?: ReactNode; secondaryActionProps?: ComponentProps<'button'>; expandableContent?: ReactNode; dismissAfterVisibleMs?: number; onClose?: () => void }`.
  - `Toaster` (bottom right, stacked); `toastManager` (for code outside React: the desktop update toast, Task 16).
  - `stackedToast(o: StackedToastOptions): ToastManagerAddOptions<ToastData>`.
  - `resolveSidebarState`, `ResponsiveSidebarState`.
  - `ConfirmDialog({ open, onClose, onConfirm, object, impact, confirm, pending }: ConfirmDialogProps)` and `TypedConfirmDialog({ open, onClose, onConfirm, object, impact, phrase, confirm, pending }: TypedConfirmDialogProps)`.

- [ ] **Step 1: Write the failing tests**

`web/packages/ui/test/toast-confirm.test.tsx`:

```tsx
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  ConfirmDialog,
  resolveSidebarState,
  Toaster,
  ToastProvider,
  toastManager,
  TypedConfirmDialog,
  useToast,
} from '../src';

afterEach(cleanup);
beforeEach(() => vi.useRealTimers());
Element.prototype.getAnimations ??= () => [];

function Raise({ onReady }: { onReady: (t: ReturnType<typeof useToast>) => void }) {
  onReady(useToast());
  return null;
}

describe('toasts (D1082)', () => {
  it('stack bottom right, with an action, and the old danger type still works', async () => {
    let toast!: ReturnType<typeof useToast>;
    const undo = vi.fn();
    render(
      <ToastProvider>
        <Raise onReady={(t) => (toast = t)} />
        <Toaster />
      </ToastProvider>,
    );
    act(() => {
      toast.add({ title: 'Branch created', type: 'success' });
      toast.add({ title: 'Delete failed', description: 'locked', type: 'danger' });
      toast.add({ title: 'Table dropped', actionProps: { children: 'Undo', onClick: undo } });
    });
    const viewport = await screen.findByRole('region', { name: /notifications/i });
    expect(viewport.className).toContain('bottom-4');
    expect(viewport.className).toContain('right-4');
    const titles = [...viewport.querySelectorAll('[data-slot="toast-title"]')].map((n) => n.textContent);
    expect(titles).toHaveLength(3);
    expect(titles).toEqual(expect.arrayContaining(['Branch created', 'Delete failed', 'Table dropped']));
    expect(viewport.querySelector('[data-type="error"]')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(undo).toHaveBeenCalled();
  });

  it('auto-dismisses after its timeout', async () => {
    vi.useFakeTimers();
    render(
      <ToastProvider>
        <Toaster />
      </ToastProvider>,
    );
    act(() => {
      toastManager.add({ title: 'Saved', timeout: 1000 });
    });
    expect(screen.getByText('Saved')).toBeTruthy();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(screen.queryByText('Saved')).toBeNull();
  });
});

describe('confirmations (D1082)', () => {
  it('a destructive confirmation names the object and its impact', async () => {
    render(
      <ConfirmDialog
        open
        onClose={() => {}}
        onConfirm={() => {}}
        object="the table orders"
        impact="Its 1,204 rows are removed from this branch."
        confirm="Drop table"
      />,
    );
    await act(async () => {});
    const dialog = screen.getByRole('alertdialog', { name: /Drop the table orders\?/ });
    expect(dialog.textContent).toContain('Its 1,204 rows are removed from this branch.');
  });

  it('typed confirmation stays disabled until the phrase is typed exactly', async () => {
    const onConfirm = vi.fn();
    render(
      <TypedConfirmDialog
        open
        onClose={() => {}}
        onConfirm={onConfirm}
        object="the table orders on production"
        impact="Production data is deleted. A branch restore is the only way back."
        phrase="production"
        confirm="Drop table"
      />,
    );
    await act(async () => {});
    const button = screen.getByRole('button', { name: 'Drop table' });
    expect(button.hasAttribute('disabled')).toBe(true);
    const input = screen.getByRole('textbox', { name: /Type production to confirm/ });
    fireEvent.change(input, { target: { value: 'Production' } });
    expect(button.hasAttribute('disabled')).toBe(true);
    fireEvent.change(input, { target: { value: 'production' } });
    expect(button.hasAttribute('disabled')).toBe(false);
    fireEvent.click(button);
    expect(onConfirm).toHaveBeenCalledOnce();
  });
});

describe('sidebarState', () => {
  it('is upstream as-is', () => {
    expect(resolveSidebarState({ isMobile: true, open: true, openMobile: false })).toBe('collapsed');
    expect(resolveSidebarState({ isMobile: false, open: true, openMobile: false })).toBe('expanded');
  });
});
```

In `test/boundaries.test.ts`, add:

```ts
  it('has a row for every design 59 primitive (D1077)', () => {
    const rows = new Set(prov.files.map((r) => r.upstreamPath));
    const ts = new Set(['dialog-styles', 'sidebarState', 'toastHelpers']);
    const expected = [
      'alert', 'alert-dialog', 'autocomplete', 'badge', 'button', 'checkbox', 'collapsible-section-header',
      'combobox', 'command', 'dialog', 'dialog-styles', 'empty', 'group', 'input', 'input-group', 'kbd',
      'menu', 'middle-truncate', 'number-field', 'popover', 'preview-card', 'radio-group', 'scroll-area',
      'select', 'separator', 'sheet', 'sidebar', 'sidebarState', 'skeleton', 'spinner', 'switch', 'table',
      'textarea', 'toast', 'toastHelpers', 'toggle-group', 'tooltip',
    ].map((n) => `apps/web/src/components/ui/${n}.${ts.has(n) ? 'ts' : 'tsx'}`);
    expect(expected.filter((p) => !rows.has(p))).toEqual([]);
  });
```

`command.tsx` merges `dialog-styles.ts` and `autocomplete.tsx` in the old rows. The new `dialog-styles.ts` and `autocomplete.tsx` rows (Tasks 4–5) satisfy this list.

- [ ] **Step 2: Run them and watch them fail**

Run: `pnpm --filter @loams/ui exec vitest run test/toast-confirm.test.tsx test/boundaries.test.ts`
Expected: FAIL, with `does not provide an export named 'ConfirmDialog'`, and the row test listing `toast.tsx`, `toastHelpers.ts` and `sidebarState.ts`.

- [ ] **Step 3: Vendor the toast**

```bash
node scripts/studio/vendor.mjs import --package web/packages/ui --from "$SRC" --group k --verdict adapt \
  --change "ported to the design 59 tokens and cx; thread routing, the composer store, MorphIcon and copy-to-clipboard removed" \
  --map apps/web/src/components/ui/toast.tsx=src/toast.tsx \
  --map apps/web/src/components/ui/toastHelpers.ts=src/toast-helpers.ts
node scripts/studio/vendor.mjs import --package web/packages/ui --from "$SRC" --group k --verdict as-is \
  --map apps/web/src/components/ui/sidebarState.ts=src/sidebar-state.ts
```

Edit `src/toast.tsx`:
- Remove `useParams`, `ScopedThreadRef`/`ThreadId`, `useComposerDraftStore`, `resolveThreadRouteTarget`, `useActiveThreadRefFromRoute`, `shouldRenderThreadScopedToast`, `CopyErrorButton` and `useCopyToClipboard`. Remove `AnchoredToastProvider` and `anchoredToastManager`, which serve composer-anchored toasts Loams does not have.
- Rename `ThreadToastData` to `ToastData`, dropping `threadRef`, `threadId`, `tooltipStyle`, `hideCopyButton` and `additionalActions`.
- Keep `buildVisibleToastLayout` and `hasVisibleToastAction` from upstream's `toast.logic.ts`, inlined into `toast.tsx`. This adds the `toast.logic.ts` path to the row's note, without its own row.
- Replace the lucide icons with local SVGs: a circle-check, circle-alert, triangle-alert and info, each a 16×16 path. `loading` uses `Spinner`.
- Apply the class table. The toast surface is `rounded-lg border border-rule bg-surface text-body text-ink shadow-pop`. Icons use `in-data-[type=error]:text-danger in-data-[type=success]:text-grow in-data-[type=warning]:text-warn in-data-[type=info]:text-info`.

The provider, the manager and the Loams-facing names:

```tsx
export const toastManager = Toast.createToastManager<ToastData>();

/** The old `type: 'danger'` is the error type. */
function normalise<T extends { type?: string }>(o: T): T {
  return o.type === 'danger' ? { ...o, type: 'error' } : o;
}

export function ToastProvider({ children, ...props }: Toast.Provider.Props) {
  return (
    <Toast.Provider toastManager={toastManager} limit={4} timeout={5000} {...props}>
      {children}
    </Toast.Provider>
  );
}

/** `add({ title, description, type, actionProps })` shows a toast; it returns its id. */
export function useToast() {
  const manager = Toast.useToastManager<ToastData>();
  return {
    add: (o: ToastOptions) => manager.add(normalise(o)),
    close: (id: string) => manager.close(id),
    update: (id: string, o: Partial<ToastOptions>) => manager.update(id, normalise(o)),
  };
}

/** Where toasts appear: the bottom right, stacked, newest on top (D1082). */
export function Toaster() {
  const { toasts } = Toast.useToastManager<ToastData>();
  return (
    <Toast.Portal>
      <Toast.Viewport
        aria-label="Notifications"
        className="fixed right-4 bottom-4 z-[80] flex w-[min(380px,calc(100vw-32px))] flex-col-reverse gap-2 [-webkit-app-region:no-drag]"
      >
        {toasts.map((t) => (
          <Toast.Root
            key={t.id}
            toast={t}
            data-type={t.type}
            className="relative flex gap-3 rounded-lg border border-rule bg-surface px-3.5 py-3 text-body text-ink shadow-pop data-ending-style:opacity-0 transition-opacity duration-(--op-duration) motion-reduce:transition-none"
          >
            <ToastBody toast={t} />
            <Toast.Close
              aria-label="Dismiss"
              className="absolute top-2 right-2 grid size-6 place-items-center rounded-sm text-muted hover:bg-raised hover:text-ink"
            >
              ×
            </Toast.Close>
          </Toast.Root>
        ))}
      </Toast.Viewport>
    </Toast.Portal>
  );
}
```

`ToastBody` is upstream's `ToastBodyContent`, with `deriveToastBodyDescriptor`, the title and description slots, `ToastDescriptionAndExpandable` and the action buttons. It renders `Toast.Title` with `data-slot="toast-title"`, and its `Toast.Action` uses `buttonClass({ size: 'xs', variant: 'primary' })`. Keep upstream's `ThreadToastVisibleAutoDismiss` as `VisibleAutoDismiss`; it pauses `dismissAfterVisibleMs` while the window is hidden or blurred. Re-export `stackedToast` and `hiddenToastActionProps` from `toast-helpers.ts`, renamed from `stackedThreadToast`.

- [ ] **Step 4: Write the confirmations**

`src/confirm.tsx`:

```tsx
// Destructive confirmations (design 59, D1082): the dialog names the object and what will
// happen to it. Typed confirmation is required for production data: the person types a
// phrase (the branch or environment name) before the action unlocks. Part 3 applies the
// rule in each flow; this is the primitive.

import { type ReactNode, useEffect, useId, useState } from 'react';
import {
  AlertDialogClose,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogPopup,
  AlertDialogRoot,
  AlertDialogTitle,
} from './alert-dialog';
import { Button } from './button';
import { InputControl } from './input';

export interface ConfirmDialogProps {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  /** What is acted on, as it reads after a verb: "the table orders". */
  object: string;
  /** What happens to it, in a sentence. */
  impact: ReactNode;
  /** The verb on the button ("Drop table"); never "OK". */
  confirm: string;
  pending?: boolean;
}

export interface TypedConfirmDialogProps extends ConfirmDialogProps {
  /** The exact text to type, case-sensitive: the branch or environment name. */
  phrase: string;
}

function verbOf(confirm: string): string {
  return confirm.split(' ')[0] ?? confirm;
}

function Frame({
  open,
  onClose,
  object,
  impact,
  confirm,
  pending = false,
  children,
  disabled,
  onConfirm,
}: ConfirmDialogProps & { children?: ReactNode; disabled?: boolean }) {
  return (
    <AlertDialogRoot
      open={open}
      onOpenChange={(next) => {
        if (!next && !pending) onClose();
      }}
    >
      <AlertDialogPopup>
        <AlertDialogHeader>
          <AlertDialogTitle>{`${verbOf(confirm)} ${object}?`}</AlertDialogTitle>
          <AlertDialogDescription>{impact}</AlertDialogDescription>
          {children}
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogClose render={<Button variant="quiet" />} disabled={pending}>
            Cancel
          </AlertDialogClose>
          <Button
            variant="danger-solid"
            onClick={onConfirm}
            disabled={pending || disabled}
            aria-busy={pending || undefined}
          >
            {confirm}
          </Button>
        </AlertDialogFooter>
      </AlertDialogPopup>
    </AlertDialogRoot>
  );
}

export function ConfirmDialog(props: ConfirmDialogProps) {
  return <Frame {...props} />;
}

export function TypedConfirmDialog({ phrase, ...props }: TypedConfirmDialogProps) {
  const [typed, setTyped] = useState('');
  const id = useId();
  useEffect(() => {
    if (!props.open) setTyped('');
  }, [props.open]);
  return (
    <Frame {...props} disabled={typed !== phrase}>
      <label htmlFor={id} className="mt-2 text-small text-muted">
        Type <code className="font-mono text-ink">{phrase}</code> to confirm
      </label>
      <InputControl
        id={id}
        aria-label={`Type ${phrase} to confirm`}
        value={typed}
        onChange={(e) => setTyped(e.currentTarget.value)}
        autoComplete="off"
        spellCheck={false}
        font="mono"
      />
    </Frame>
  );
}
```

The test looks the dialog up by the title "Drop the table orders?". `verbOf("Drop table")` gives "Drop", and the object is "the table orders". Export `ConfirmDialog`, `ConfirmDialogProps`, `TypedConfirmDialog`, `TypedConfirmDialogProps`, `toastManager`, `stackedToast`, `ToastData`, `ToastOptions`, `resolveSidebarState` and `ResponsiveSidebarState` from `index.ts`.

- [ ] **Step 5: Run the tests**

```bash
pnpm --filter @loams/ui test
node scripts/studio/vendor.mjs check --package web/packages/ui
pnpm -C web -r --if-present test
```

Expected: PASS. The as-is row's `localSha256` matches the file with its header.

- [ ] **Step 6: Commit**

```bash
git add web/packages/ui
git commit -s -m "feat(ui): stacked toasts, destructive and typed confirmation (CF1 task 6)"
```

---

## CF1c — The shell regions (D1078–D1082)

### Task 7: The frame: sidebar, top bar (the drag region) and main

**Files:**
- Modify: `web/packages/slots/src/types.ts` (the `shell.sidebar.*`, `shell.topbar.*` and `shell.home` slots), `src/registry.ts` (`SLOT_KINDS`)
- Modify: `web/packages/slots/test/slots.test.tsx`
- Create: `web/plugins/shell/src/frame.tsx` (`SIDEBAR`, `useSidebarState`, `useNarrow`, `TopBar`, `MainRegion`, `runShellCommand`; `Crumbs`, `Trail`, `Outlet`, `UserMenu`, `HeaderContext` and `Brand` move here from `layout.tsx` unchanged)
- Create: `web/plugins/shell/src/sidebar.tsx` (`ShellSidebar`; the existing `Nav` and `EnvironmentSwitcher` move here unchanged until Task 10)
- Create: `web/plugins/shell/src/nav.ts` (`inMode`, `ModeFilter`, `navSections`, `SECTIONS`, `STUDIO_SECTIONS` and `LEGACY` move here from `layout.tsx`, which re-exports them; this breaks the frame ↔ layout import cycle)
- Modify: `web/plugins/shell/src/layout.tsx` (the `Layout` body), `src/palette.tsx` (`ShellPalette` takes optional `open`/`onOpenChange`), `src/index.tsx` (exports), `package.json` (`loams.plugin.slots`)
- Modify: `web/apps/console/src/cordis/console.css:38-598` (the `/* Frame */` section; `--top: 44px`)
- Modify: `web/plugins/desktop-servers/src/index.tsx:35`, `web/apps/console/src/hosted/edition.tsx:174` (`shell.header.server` → `shell.sidebar.server`)
- Modify: `web/plugins/factory/src/embedded.tsx:157`, `web/plugins/studio/src/layout/StudioLayout.tsx:224` (fill the region), `web/packages/admin-ui/src/admin.css:512` (marker; T7-1)
- Create: `web/plugins/shell/test/frame.test.tsx`, `web/plugins/shell/test/viewport.test.ts` (Review Focus 4)
- Modify: `web/plugins/shell/test/layout.test.tsx` (header and sidebar assertions move to `frame.test.tsx`)

**Interfaces:**
- Consumes: `ThemeService` and `AppearanceMenu` (Task 2); `Sidebar`, `SidebarContent`, `SidebarFooter`, `SidebarHeader` and `Sheet` from `@loams/ui`; `SHELL_COMMAND_EVENT`, `MODE_HOME` and `ModeService` from `@loams/console-host`.
- Produces:
  - Slots: `shell.sidebar.server`, `shell.sidebar.threads`, `shell.topbar.left`, `shell.topbar.center`, `shell.topbar.right`, `shell.home`.
  - `SIDEBAR` (`{ key: 'loams.shell.sidebar', min: 200, max: 320, size: 232 }`).
  - `useSidebarState(): [{ open: boolean; width: number }, (patch: Partial<{ open: boolean; width: number }>) => void]` and `useNarrow(): boolean` (true under 960px).
  - `runShellCommand(detail: string, fallback: () => void, target?: EventTarget): void`.
  - `TopBar`, `MainRegion({ router, mode }: { router: HashRouter; mode?: ModeService })` and `ShellSidebar`.
  - The CSS contract: `.lc-topbar` is `-webkit-app-region: drag`; a control in it is `no-drag`; `--lc-traffic` and `--lc-wco-end` reserve OS window-control space (Task 13 sets them).

- [ ] **Step 1: Write the failing tests**

`web/plugins/shell/test/viewport.test.ts` (Review Focus 4):

```ts
// @vitest-environment node
// Review Focus 4: a page sizes itself to the main region, never against the viewport or the
// old 56px header, now that a 44px top bar, an inspector and a drawer share the window. A bare
// page (sign-in, setup) may fill the viewport with a `viewport-ok: bare page` marker.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const WEB = fileURLToPath(new URL('../../..', import.meta.url));
const ROOTS = ['plugins', 'packages/studio-ui', 'packages/admin-ui', 'packages/desktop-ui'].map((d) => join(WEB, d));
const SKIP = new Set(['node_modules', 'dist', 'gen', 'test', 'tests']);
const VIEWPORT = /\b100(?:s|d|l)?vh\b|var\(--top,\s*56px\)/;

function* files(dir: string): Generator<string> {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name) || name.startsWith('.')) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* files(p);
    else if (/\.(tsx?|css)$/.test(name) && !/\.(test|spec)\./.test(name)) yield p;
  }
}

describe('pages fill the main region (Review Focus 4)', () => {
  it('no page sizes itself against the viewport', () => {
    const hits: string[] = [];
    for (const root of ROOTS) {
      for (const path of files(root)) {
        if (path.includes(`${join('plugins', 'shell')}`)) continue;
        readFileSync(path, 'utf8')
          .split('\n')
          .forEach((line, i) => {
            if (VIEWPORT.test(line) && !line.includes('viewport-ok:')) hits.push(`${relative(WEB, path)}:${i + 1}`);
          });
      }
    }
    expect(hits).toEqual([]);
  });
});
```

`web/plugins/shell/test/frame.test.tsx`:

```tsx
import { readFileSync } from 'node:fs';
import { type SessionService, ThemeController } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { SIDEBAR } from '../src/frame.js';
import { Layout } from '../src/layout.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  return { get: () => path, set: (p) => void (path = p), listen: () => () => {} };
}
const session = { principal: () => ({ displayName: 'Omar' }), subscribe: () => () => {} } as unknown as SessionService;
const theme = new ThemeController({ read: () => 'dark', write: () => {} });

function setup(start = '/') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  const ui = render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} theme={theme} />
    </SlotProvider>,
  );
  return { slots, router, ui };
}

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the frame (D1078)', () => {
  it('has a sidebar, a 44px top bar that is the drag region, and main', () => {
    setup();
    const bar = document.querySelector('.lc-topbar') as HTMLElement;
    expect(bar).toBeTruthy();
    expect(bar.dataset.dragRegion).toBe('');
    expect(screen.getByRole('main')).toBeTruthy();
    expect(screen.getByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
    // jsdom does not load console.css, so the drag contract is checked in the stylesheet.
    const css = readFileSync(new URL('../../../apps/console/src/cordis/console.css', import.meta.url), 'utf8');
    expect(css).toMatch(/\.lc-topbar \{[^}]*height: var\(--op-topbar\);[^}]*-webkit-app-region: drag;/);
    expect(css).toMatch(/\.lc-topbar :is\(button, a, input, select, textarea, \[role="button"\], \[role="combobox"\], \[data-no-drag\]\) \{\s*-webkit-app-region: no-drag;/);
  });

  it('collapses and restores the sidebar, and keeps its width within 200–320', () => {
    localStorage.setItem(SIDEBAR.key, JSON.stringify({ open: true, width: 900 }));
    setup();
    const nav = screen.getByRole('navigation', { name: 'Sidebar' });
    expect((nav.parentElement as HTMLElement).style.width).toBe('320px');
    fireEvent.click(screen.getByRole('button', { name: 'Hide sidebar' }));
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
    expect(JSON.parse(localStorage.getItem(SIDEBAR.key) ?? '{}').open).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: 'Show sidebar' }));
    expect(screen.getByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
  });

  it('renders the server indicator in the sidebar and the top bar slots in order', () => {
    const { slots } = setup();
    act(() => {
      slots.register({ name: 'shell.sidebar.server', plugin: 't' }, () => <span>Local engine</span>);
      slots.register({ name: 'shell.topbar.left', plugin: 't' }, () => <button type="button">acme</button>);
      slots.register({ name: 'shell.topbar.right', plugin: 't' }, () => <span>right</span>);
    });
    expect(screen.getByRole('navigation', { name: 'Sidebar' }).textContent).toContain('Local engine');
    expect((document.querySelector('.lc-topbar') as HTMLElement).textContent).toMatch(/acme[\s\S]*right/);
  });

  it('renders the home slot on the mode home and the page outlet elsewhere', () => {
    const { slots, router } = setup('/studio');
    act(() => {
      slots.register({ name: 'shell.home', plugin: 'studio' }, () => <p>composer</p>);
      router.page({ id: 'studio', path: '/studio', title: 'Studio' }, () => <p>old studio page</p>);
      router.page({ id: 'data', path: '/data', title: 'Data' }, () => <p>data page</p>);
    });
    // Without a mode service the home is Console's "/": /studio renders its page.
    expect(screen.getByText('old studio page')).toBeTruthy();
    act(() => router.navigate('/data'));
    expect(screen.getByText('data page')).toBeTruthy();
  });

  it('New thread asks Studio first, then falls back to its home', () => {
    const { router } = setup('/data');
    fireEvent.click(screen.getByRole('button', { name: /New thread/ }));
    expect(router.current().path).toBe('/studio');
  });
});
```

The home-slot case with a mode service (`MODE_HOME.studio`) is covered in Task 9, where Studio registers it. In `web/packages/slots/test/slots.test.tsx`, add:

```tsx
  it('knows the design 59 shell slots', () => {
    const r = new SlotRegistry();
    for (const name of ['shell.sidebar.server', 'shell.sidebar.threads', 'shell.topbar.left', 'shell.topbar.center', 'shell.topbar.right'] as const) {
      expect(() => r.register({ name, plugin: 't' }, () => null)).not.toThrow();
    }
    r.register({ name: 'shell.home', plugin: 'a' }, () => null);
    expect(() => r.register({ name: 'shell.home', plugin: 'b' }, () => null)).toThrow(/single/);
  });
```

- [ ] **Step 2: Run them and watch them fail**

```bash
pnpm --filter @loams/plugin-shell exec vitest run test/frame.test.tsx test/viewport.test.ts
pnpm --filter @loams/slots test
```

Expected: FAIL. The viewport test lists `plugins/factory/src/embedded.tsx:157`, `plugins/studio/src/layout/StudioLayout.tsx:224` and `packages/admin-ui/src/admin.css:512`. The frame test fails with `Cannot find module '../src/frame.js'`. The slots test fails with `unknown slot shell.sidebar.server`.

- [ ] **Step 3: Declare the slots**

In `types.ts`, add to `SlotMap`:

```ts
  /** The server indicator under the brand (design 59 D1078): the desktop's server switcher. */
  'shell.sidebar.server': { kind: 'list'; props: Record<string, never> };
  /** Projects and their threads (Studio's sidebar, W6); empty hides the section. */
  'shell.sidebar.threads': { kind: 'list'; props: Record<string, never> };
  /** The top bar's start: the project and branch pills. Entries filter by `meta.mode`. */
  'shell.topbar.left': { kind: 'list'; props: Record<string, never> };
  'shell.topbar.center': { kind: 'list'; props: Record<string, never> };
  /** Before the inspector and drawer toggles. */
  'shell.topbar.right': { kind: 'list'; props: Record<string, never> };
  /** The agent home (Part 2 fills it; W11). Rendered on the mode's home route with its params. */
  'shell.home': { kind: 'single'; props: { params: Record<string, string> } };
```

Add each to `SLOT_KINDS` with the same kind, and add the five list slots and `shell.home` to the shell's `loams.plugin.slots` in `package.json`.

- [ ] **Step 4: Build the frame**

`web/plugins/shell/src/frame.tsx`. The moved helpers stay as they are; the new parts are these:

```tsx
// The shell's frame (design 59, D1078): the top bar is the window's drag region (D1083), the
// main region renders the home slot on the mode's home and a page everywhere else.

import { MODE_HOME, type ModeService, SHELL_COMMAND_EVENT, type ThemeService } from '@loams/console-host';
import { Slot, SlotBoundary, useSlot } from '@loams/slots';
import { Kbd, KbdGroup } from '@loams/ui';
import { PanelLeft } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useState, useSyncExternalStore } from 'react';
import { inMode } from './nav.js';
import type { HashRouter } from './router.js';

export const SIDEBAR = { key: 'loams.shell.sidebar', min: 200, max: 320, size: 232 } as const;

interface SidebarState {
  open: boolean;
  width: number;
}

const clampSidebar = (n: number) => Math.min(SIDEBAR.max, Math.max(SIDEBAR.min, Math.round(n)));

export function readSidebar(): SidebarState {
  try {
    const raw = JSON.parse(localStorage.getItem(SIDEBAR.key) ?? 'null') as Partial<SidebarState> | null;
    return {
      open: raw?.open !== false,
      width: typeof raw?.width === 'number' ? clampSidebar(raw.width) : SIDEBAR.size,
    };
  } catch {
    return { open: true, width: SIDEBAR.size };
  }
}

export function useSidebarState(): [SidebarState, (patch: Partial<SidebarState>) => void] {
  const [state, setState] = useState(readSidebar);
  const update = useCallback((patch: Partial<SidebarState>) => {
    setState((prev) => {
      const next = { ...prev, ...patch, width: clampSidebar(patch.width ?? prev.width) };
      try {
        localStorage.setItem(SIDEBAR.key, JSON.stringify(next));
      } catch {
        // Storage blocked: the sidebar still works this session.
      }
      return next;
    });
  }, []);
  return [state, update];
}

/** Under 960px the sidebar is a sheet and the inspector an overlay. */
export function useNarrow(): boolean {
  const query = '(max-width: 959px)';
  return useSyncExternalStore(
    (cb) => {
      const mq = globalThis.matchMedia?.(query);
      mq?.addEventListener?.('change', cb);
      return () => mq?.removeEventListener?.('change', cb);
    },
    () => globalThis.matchMedia?.(query).matches ?? false,
    () => false,
  );
}

/** Sends a shell command to whoever handles it (Studio's `chat.new`); else runs `fallback`. */
export function runShellCommand(detail: string, fallback: () => void, target: EventTarget = globalThis): void {
  const event = new CustomEvent(SHELL_COMMAND_EVENT, { detail, cancelable: true });
  target.dispatchEvent(event);
  if (!event.defaultPrevented) fallback();
}

function useLocation(router: HashRouter) {
  return useSyncExternalStore(router.subscribe, () => router.current());
}

function Entries({ name, mode }: { name: 'shell.topbar.left' | 'shell.topbar.center' | 'shell.topbar.right' | 'shell.header.server'; mode?: ModeService }) {
  const entries = useSlot(name);
  const current = useSyncExternalStore(
    (cb) => mode?.onChange(cb) ?? (() => {}),
    () => mode?.current,
  );
  const filter = mode && current ? { mode: current, modesOf: (id: string) => mode.modesOf(id) } : undefined;
  return (
    <>
      {entries
        .filter((e) => inMode(e, filter))
        .map((entry) => {
          const Entry = entry.component;
          return (
            <SlotBoundary key={entry.id} plugin={entry.plugin}>
              <Entry />
            </SlotBoundary>
          );
        })}
    </>
  );
}

export function TopBar({
  router,
  flags,
  mode,
  sidebarOpen,
  onToggleSidebar,
  onOpenPalette,
  tools,
  trailing,
}: {
  router: HashRouter;
  flags?: import('@loams/console-host').FlagsService;
  mode?: ModeService;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  onOpenPalette: () => void;
  /** The mode switcher, the appearance menu and the account (Layout's). */
  tools: ReactNode;
  /** The inspector and drawer toggles (Task 8). */
  trailing?: ReactNode;
}) {
  const mac = /mac|iphone|ipad/i.test(globalThis.navigator?.platform ?? '');
  return (
    <header className="lc-topbar" data-drag-region="">
      <button
        type="button"
        className="lc-icon-btn lc-sidebar-toggle"
        aria-label={sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}
        aria-controls="lc-side"
        aria-expanded={sidebarOpen}
        aria-keyshortcuts="Control+B Meta+B"
        title={`${sidebarOpen ? 'Hide' : 'Show'} sidebar (${mac ? '⌘' : 'Ctrl+'}B)`}
        onClick={onToggleSidebar}
      >
        <PanelLeft size={16} strokeWidth={1.75} aria-hidden="true" />
      </button>
      <div className="lc-topbar-start">
        <Entries name="shell.topbar.left" mode={mode} />
        <Entries name="shell.header.server" mode={mode} />
        <Crumbs router={router} flags={flags} mode={mode} />
      </div>
      <div className="lc-topbar-center">
        <Entries name="shell.topbar.center" mode={mode} />
        <button type="button" className="lc-palette-hint" onClick={onOpenPalette} aria-keyshortcuts="Control+K Meta+K">
          <span>Search or run a command</span>
          <KbdGroup>
            <Kbd>{mac ? '⌘' : 'Ctrl'}</Kbd>
            <Kbd>K</Kbd>
          </KbdGroup>
        </button>
      </div>
      <div className="lc-topbar-end">
        <Entries name="shell.topbar.right" mode={mode} />
        <div className="lc-overlays">
          <Slot name="shell.overlay" props={{}} />
        </div>
        {tools}
        {trailing}
      </div>
    </header>
  );
}

/** The home slot on the mode's home route (W11); the page outlet everywhere else. */
export function MainRegion({ router, mode }: { router: HashRouter; mode?: ModeService }) {
  const location = useLocation(router);
  const current = useSyncExternalStore(
    (cb) => mode?.onChange(cb) ?? (() => {}),
    () => mode?.current ?? 'console',
  );
  // Studio's home entry is `meta.mode: 'studio'`, so Console's "/" stays the Overview page.
  const home = useSlot('shell.home').filter((e) =>
    inMode(e, mode ? { mode: current, modesOf: (id) => mode.modesOf(id) } : undefined),
  );
  if (home.length > 0 && location.path === MODE_HOME[current]) {
    return (
      <div className="lc-home">
        <Slot name="shell.home" props={{ params: location.params }} />
      </div>
    );
  }
  return <Outlet router={router} />;
}
```

`web/plugins/shell/src/sidebar.tsx` (Task 10 replaces the `Nav` block with Pages and Settings):

```tsx
import type { FlagsService, ModeService, SessionService } from '@loams/console-host';
import { MODE_HOME } from '@loams/console-host';
import { Slot, useSlot } from '@loams/slots';
import { Sheet, Sidebar, SidebarContent, SidebarFooter, SidebarHeader } from '@loams/ui';
import { BookOpen, SquarePen } from 'lucide-react';
import { Brand, runShellCommand } from './frame.js';
import type { HashRouter } from './router.js';

export function ShellSidebar({
  router,
  flags,
  mode,
  session,
  width,
  onWidth,
  open,
  narrow,
  onClose,
}: {
  router: HashRouter;
  flags?: FlagsService;
  mode?: ModeService;
  session: SessionService;
  width: number;
  onWidth: (w: number) => void;
  open: boolean;
  narrow: boolean;
  onClose: () => void;
}) {
  const hasThreads = useSlot('shell.sidebar.threads').length > 0;
  const newThread = () =>
    runShellCommand('chat.new', () =>
      mode ? mode.set('studio', { navigate: MODE_HOME.studio }) : router.navigate(MODE_HOME.studio),
    );
  const body = (
    <>
      <SidebarHeader className="lc-side-head">
        <Brand flags={flags} home={MODE_HOME[mode?.current ?? 'console']} onClose={onClose} />
        <Slot name="shell.sidebar.server" props={{}} />
        <button type="button" className="lc-new-thread" onClick={newThread} aria-keyshortcuts="Control+N Meta+N">
          <SquarePen size={15} strokeWidth={1.75} aria-hidden="true" /> New thread
        </button>
      </SidebarHeader>
      <SidebarContent>
        {hasThreads && (
          <section aria-label="Threads" className="lc-side-threads">
            <Slot name="shell.sidebar.threads" props={{}} />
          </section>
        )}
        <Nav router={router} mode={mode} session={session} />
      </SidebarContent>
      <SidebarFooter className="lc-side-foot">
        <a href="https://loams.dev/docs" target="_blank" rel="noreferrer">
          <BookOpen size={15} aria-hidden="true" /> Documentation
        </a>
        <span className="lc-side-version">Loams {flags?.serverVersion || 'console'}</span>
      </SidebarFooter>
    </>
  );
  if (narrow) {
    return (
      <Sheet open={open} onClose={onClose} title="Navigation" side="left">
        <nav aria-label="Sidebar" className="flex min-h-0 flex-1 flex-col">
          {body}
        </nav>
      </Sheet>
    );
  }
  return (
    <Sidebar label="Sidebar" width={width} onWidthChange={onWidth} min={200} max={320} open={open} className="lc-side">
      {body}
    </Sidebar>
  );
}
```

`Nav` is the existing `Nav` from `layout.tsx`, moved here. Its `session` prop renders `EnvironmentSwitcher` or `shell.nav.context` above the sections, as the old `Layout` did (layout.tsx:713-717).

`@loams/ui`'s `Sidebar` renders `<nav aria-label={label}>` inside a `div` with `style={{ width }}`; the frame test relies on that. In `Layout`, replace the old `lc-shell` tree (layout.tsx:706-806):

```tsx
  const [side, setSide] = useSidebarState();
  const narrow = useNarrow();
  const [sheetOpen, setSheetOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const sidebarOpen = narrow ? sheetOpen : side.open;
  const toggleSidebar = () => (narrow ? setSheetOpen((v) => !v) : setSide({ open: !side.open }));
  // The gate, the engine check and the bare-page branch (layout.tsx:693-704) stay as they are.
  return (
    <ToastProvider>
      <TooltipProvider>
        <div className="lc-shell" data-mode={current} data-sidebar={sidebarOpen ? 'open' : 'closed'}>
          <ShellSidebar
            router={router}
            flags={flags}
            mode={mode}
            session={session}
            width={side.width}
            onWidth={(w) => setSide({ width: w })}
            open={sidebarOpen}
            narrow={narrow}
            onClose={() => (narrow ? setSheetOpen(false) : setSide({ open: false }))}
          />
          <div className="lc-body">
            <TopBar
              router={router}
              flags={flags}
              mode={mode}
              sidebarOpen={sidebarOpen}
              onToggleSidebar={toggleSidebar}
              onOpenPalette={() => setPaletteOpen(true)}
              tools={
                <>
                  {mode && <ModeSwitcher mode={mode} />}
                  <AppearanceMenu theme={theme} />
                  <UserMenu session={session} router={router} local={flags?.features.local} />
                </>
              }
            />
            <div className="lc-work">
              <div className="lc-work-row">
                <main className="lc-main" id="lc-main">
                  <MainRegion router={router} mode={mode} />
                </main>
                {/* The agent dock stays here until Task 8 turns it into the inspector. */}
                {hasDock && mounted && (
                  <aside className="lc-dock" id="lc-dock" aria-label="Agent panel" ref={dockRef} tabIndex={-1} hidden={!open}>
                    <DockHandle width={dock.width} onWidth={(w) => setDock({ width: clampWidth(w) })} />
                    <div className="lc-dock-body">
                      <Slot name="shell.dock.right" props={{}} />
                    </div>
                  </aside>
                )}
              </div>
            </div>
          </div>
        </div>
        <Palette router={router} mode={mode} theme={theme} dock={hasDock ? { open, toggle } : undefined} open={paletteOpen} onOpenChange={setPaletteOpen} />
        <Toaster />
      </TooltipProvider>
    </ToastProvider>
  );
```

`Palette` passes `open` and `onOpenChange` to `ShellPalette`. `ShellPalette` uses them when given, and keeps its own state otherwise. Its Mod+K listener calls `onOpenChange(!open)`.

- [ ] **Step 5: The frame CSS**

In `web/apps/console/src/cordis/console.css`, replace the `/* Frame */` section through `.lc-dock-handle:hover` (lines 40–590). Keep the `.lc-nav*`, `.lc-trail`, `.lc-switcher`, `.lc-user-btn` and `.lc-boot` blocks, restyled to the type scale. The new frame:

```css
/* Frame (design 59, D1078): the window holds the sidebar and the top bar; the work area
   (main, inspector, drawer) is a card on it. */
.lc-shell {
  --top: var(--op-topbar);
  --lc-traffic: 0px; /* macOS traffic lights (Task 13) */
  --lc-wco-end: 0px; /* Windows/Linux title bar controls (Task 13) */
  display: flex;
  height: 100svh; /* viewport-ok: the shell itself */
  overflow: hidden;
  background: var(--op-window);
  color: var(--op-ink);
  font-size: var(--op-text-body);
}
.lc-side {
  background: var(--op-window);
}
.lc-side-head {
  border-bottom: 0;
  padding-top: max(8px, var(--lc-traffic-top, 0px));
}
.lc-new-thread {
  display: flex;
  align-items: center;
  gap: 8px;
  height: var(--op-row);
  padding: 0 8px;
  border: 0;
  border-radius: var(--op-radius-md);
  background: none;
  color: var(--op-ink);
  font: inherit;
  font-size: var(--op-text-small);
  font-weight: 500;
  cursor: pointer;
}
.lc-new-thread:hover {
  background: var(--op-row-hover);
}
.lc-body {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
}
.lc-topbar {
  display: flex;
  flex: none;
  align-items: center;
  gap: 8px;
  height: var(--op-topbar);
  padding: 0 calc(12px + var(--lc-wco-end)) 0 12px;
  -webkit-app-region: drag;
  user-select: none;
}
.lc-shell[data-sidebar="closed"] .lc-topbar {
  padding-inline-start: calc(12px + var(--lc-traffic));
}
.lc-topbar :is(button, a, input, select, textarea, [role="button"], [role="combobox"], [data-no-drag]) {
  -webkit-app-region: no-drag;
}
.lc-topbar-start,
.lc-topbar-end {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.lc-topbar-start {
  flex: 1;
}
.lc-topbar-center {
  display: flex;
  justify-content: center;
}
.lc-palette-hint {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  height: var(--op-control-compact);
  padding: 0 6px 0 10px;
  border: 1px solid var(--op-rule);
  border-radius: var(--op-radius-md);
  background: var(--op-bg);
  color: var(--op-muted);
  font: inherit;
  font-size: var(--op-text-small);
  cursor: pointer;
}
.lc-work {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  margin: 0 8px 8px 0;
  overflow: hidden;
  border: 1px solid var(--op-rule);
  border-radius: var(--op-radius-lg);
  background: var(--op-bg);
}
.lc-shell[data-sidebar="closed"] .lc-work {
  margin-left: 8px;
}
.lc-work-row {
  display: flex;
  flex: 1;
  min-height: 0;
}
.lc-main {
  flex: 1;
  min-width: 0;
  overflow: auto;
}
.lc-home {
  display: grid;
  min-height: 100%;
  place-items: center;
  padding: 24px 20px;
}
@media (max-width: 959px) {
  .lc-work {
    margin: 0;
    border-width: 1px 0 0;
    border-radius: 0;
  }
  .lc-palette-hint span {
    display: none;
  }
}
```

The old `.lc-dock*` rules stay until Task 8, with `top: var(--top)` removed and `height: 100%` in place of `calc(100svh - var(--top))`.

- [ ] **Step 6: Move the server indicators and fix the viewport sizing**

- `web/plugins/desktop-servers/src/index.tsx:35` and `web/apps/console/src/hosted/edition.tsx:174`: change `name: 'shell.header.server'` to `name: 'shell.sidebar.server'`. Studio's own header entry moves in Task 9.
- `web/plugins/factory/src/embedded.tsx:157`: delete `style={{ height: 'calc(100vh - 190px)' }}` and add `flex-1` to the placeholder's class list. Give the wrapper `<div>` above it `flex h-full min-h-0 flex-col`.
- `web/plugins/studio/src/layout/StudioLayout.tsx:224`: `h-[calc(100svh-var(--top,56px))]` → `h-full`.
- `web/packages/admin-ui/src/admin.css:512`: append the marker `/* viewport-ok: bare page (sign-in) */` to `min-height: 100svh;`.

- [ ] **Step 7: Run the tests**

```bash
pnpm --filter @loams/slots test
pnpm --filter @loams/plugin-shell test
pnpm -C web -r --if-present test
pnpm -C web typecheck
```

Expected: PASS. Update `layout.test.tsx`: the old header assertions (`lc-header`, the "Open navigation" button) move to `frame.test.tsx`'s top bar and sidebar toggle, and the dock tests keep passing unchanged until Task 8.

- [ ] **Step 8: Look at it**

Run `pnpm --filter @loams/console dev` and open `/ui/?demo` and `/ui/?desktop` in both themes. Check that:
- the top bar is 44px;
- the sidebar collapses and its width drags between 200 and 320px;
- below 960px the sidebar is a sheet;
- every page scrolls inside the work card;
- Studio fills it without a second scrollbar.

- [ ] **Step 9: Commit**

```bash
git add web/packages/slots web/plugins/shell web/plugins/desktop-servers web/plugins/factory web/plugins/studio \
  web/packages/admin-ui web/apps/console
git commit -s -m "feat(shell): the design 59 frame: sidebar, top bar drag region and main (CF1 task 7)"
```

---

### Task 8: The inspector and the drawer, and `inspector.open()`

**Files:**
- Modify: `web/packages/console-host/src/services.ts` (`PanelState`, `PanelService`; `Services.inspector`, `Services.drawer`), `src/index.ts`
- Modify: `web/packages/slots/src/types.ts` (`PanelTabProps`; the `shell.inspector.tab` and `shell.drawer.tab` slots; `SlotMeta.routes`), `src/registry.ts`, `src/index.ts` (export `PanelTabProps`)
- Create: `web/plugins/shell/src/panel.ts` (`PanelConfig`, `INSPECTOR`, `DRAWER`, `PanelController`)
- Create: `web/plugins/shell/src/regions.tsx` (`PanelRegion`, `PanelToggles`, `tabShows`)
- Modify: `web/plugins/shell/src/layout.tsx` (the dock, `DockHandle`, `useDock`, `DOCK_*` and the old ⌘J listener are removed; the regions mount), `src/index.tsx` (provides `inspector` and `drawer`), `package.json` (`provides`, `slots`)
- Modify: `web/plugins/agent/src/index.tsx:57` (`shell.dock.right` → `shell.inspector.tab`, `meta: { id: 'agent', label: 'Agent' }`), `web/plugins/agent/package.json` (`slots`)
- Modify: `web/apps/console/src/cordis/console.css` (the `.lc-dock*` rules become `.lc-inspector*` and `.lc-drawer*`)
- Create: `web/plugins/shell/test/regions.test.tsx`
- Modify: `web/plugins/shell/test/layout.test.tsx` (the dock tests are deleted; `regions.test.tsx` covers them)

**Interfaces:**
- Consumes: `ResizeHandle`, `Tabs` and `Sheet` from `@loams/ui`; `compile` from `src/router.ts`; `useNarrow` (Task 7).
- Produces:
  - `PanelState` and `PanelService` (Shared contracts); `PanelTabProps`; `SlotMeta.routes`.
  - `PanelController` with `INSPECTOR` (240–640, 280) and `DRAWER` (160–560, 280).
  - `PanelRegion({ kind, panel, router }: { kind: 'inspector' | 'drawer'; panel: PanelService; router: HashRouter })` and `PanelToggles({ inspector, drawer, router })`.
  - `tabShows(entry: Pick<SlotEntry, 'meta'>, path: string): boolean`.
  - The services `inspector` and `drawer`.

- [ ] **Step 1: Write the failing tests**

`web/plugins/shell/test/regions.test.tsx`:

```tsx
import { type SessionService, ThemeController } from '@loams/console-host';
import { type PanelTabProps, SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Layout } from '../src/layout.js';
import { DRAWER, INSPECTOR, PanelController } from '../src/panel.js';
import { tabShows } from '../src/regions.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  const listeners = new Set<() => void>();
  return {
    get: () => path,
    set: (p) => {
      path = p;
      for (const l of listeners) l();
    },
    listen: (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
}
const session = { principal: () => null, subscribe: () => () => {} } as unknown as SessionService;
const theme = new ThemeController({ read: () => 'dark', write: () => {} });

function setup(start = '/') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  const inspector = new PanelController(INSPECTOR);
  const drawer = new PanelController(DRAWER);
  render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} theme={theme} inspector={inspector} drawer={drawer} />
    </SlotProvider>,
  );
  return { slots, router, inspector, drawer };
}

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('PanelController', () => {
  it('opens a tab with props, toggles, clamps its size and persists all but the props', () => {
    const p = new PanelController(INSPECTOR);
    p.open({ tab: 'studio.diff', props: { file: 'a.ts' } });
    expect(p.state()).toMatchObject({ open: true, tab: 'studio.diff', props: { file: 'a.ts' }, size: 280 });
    p.toggle('studio.files');
    expect(p.state()).toMatchObject({ open: true, tab: 'studio.files', props: {} });
    p.toggle('studio.files');
    expect(p.state().open).toBe(false);
    p.resize(10_000);
    expect(p.state().size).toBe(640);
    p.resize(10);
    expect(p.state().size).toBe(240);
    const saved = JSON.parse(localStorage.getItem(INSPECTOR.key) ?? '{}');
    expect(saved).toEqual({ open: false, tab: 'studio.files', size: 240 });
  });

  it('survives a corrupt saved state', () => {
    localStorage.setItem(DRAWER.key, '{not json');
    expect(new PanelController(DRAWER).state()).toEqual({ open: false, tab: undefined, props: {}, size: 280 });
  });
});

describe('tab routes', () => {
  it('a tab with routes shows only on matching routes', () => {
    const entry = { meta: { routes: ['/studio/t/:threadId'] } };
    expect(tabShows(entry, '/studio/t/c_1')).toBe(true);
    expect(tabShows(entry, '/data')).toBe(false);
    expect(tabShows({ meta: {} }, '/data')).toBe(true);
  });
});

describe('the inspector (D1078)', () => {
  it('inspector.open({ tab, props }) shows that tab with its props, as "Open ↗" does', () => {
    const { slots, inspector } = setup('/data');
    act(() => {
      slots.register({ name: 'shell.inspector.tab', plugin: 't', meta: { id: 'details', label: 'Details' } }, ({ props }: PanelTabProps) => (
        <p>details of {String(props.table ?? 'nothing')}</p>
      ));
      slots.register({ name: 'shell.inspector.tab', plugin: 't', meta: { id: 'sql', label: 'SQL' } }, () => <p>sql tab</p>);
    });
    act(() => inspector.open({ tab: 'details', props: { table: 'orders' } }));
    const region = screen.getByRole('complementary', { name: 'Inspector' });
    expect(region.textContent).toContain('details of orders');
    fireEvent.click(screen.getByRole('tab', { name: 'SQL' }));
    expect(screen.getByText('sql tab')).toBeTruthy();
  });

  it('hides tabs whose routes do not match, and its toggle when none apply', () => {
    const { slots, router } = setup('/data');
    act(() => {
      slots.register(
        { name: 'shell.inspector.tab', plugin: 'studio', meta: { id: 'studio.diff', label: 'Diff', routes: ['/studio/t/:threadId'] } },
        () => <p>diff</p>,
      );
    });
    expect(screen.queryByRole('button', { name: /Inspector/ })).toBeNull();
    act(() => router.navigate('/studio/t/c_1'));
    fireEvent.click(screen.getByRole('button', { name: 'Show inspector' }));
    expect(screen.getByRole('tab', { name: 'Diff' })).toBeTruthy();
  });

  it("keeps the agent tab mounted, so the agent's chat state survives closing it", () => {
    const { slots, inspector } = setup('/');
    function Chat() {
      const [n, setN] = useState(0);
      return (
        <button type="button" onClick={() => setN(n + 1)}>
          sent {n}
        </button>
      );
    }
    act(() => {
      slots.register({ name: 'shell.inspector.tab', plugin: 'agent', meta: { id: 'agent', label: 'Agent' } }, Chat);
    });
    act(() => inspector.open({ tab: 'agent' }));
    fireEvent.click(screen.getByRole('button', { name: 'sent 0' }));
    act(() => inspector.close());
    act(() => inspector.open({ tab: 'agent' }));
    expect(screen.getByRole('button', { name: 'sent 1' })).toBeTruthy();
  });

  it('resizes with the keyboard within 240–640', () => {
    const { slots, inspector } = setup('/');
    act(() => {
      slots.register({ name: 'shell.inspector.tab', plugin: 't', meta: { id: 'x', label: 'X' } }, () => <p>x</p>);
    });
    act(() => inspector.open({ tab: 'x' }));
    const handle = screen.getByRole('separator', { name: 'Resize inspector' });
    fireEvent.keyDown(handle, { key: 'End' });
    expect(inspector.state().size).toBe(640);
  });
});

describe('the drawer (D1078)', () => {
  it('opens below main with its tabs', () => {
    const { slots, drawer } = setup('/');
    act(() => {
      slots.register({ name: 'shell.drawer.tab', plugin: 't', meta: { id: 'logs', label: 'Logs' } }, () => <p>log lines</p>);
    });
    fireEvent.click(screen.getByRole('button', { name: 'Show drawer' }));
    expect(drawer.state().open).toBe(true);
    expect(screen.getByRole('region', { name: 'Drawer' }).textContent).toContain('log lines');
  });
});
```

- [ ] **Step 2: Run them and watch them fail**

Run: `pnpm --filter @loams/plugin-shell exec vitest run test/regions.test.tsx`
Expected: FAIL, with `Cannot find module '../src/panel.js'`.

- [ ] **Step 3: The contracts**

In `console-host/src/services.ts`, add `PanelState` and `PanelService` exactly as in "Shared contracts", and add `inspector: PanelService; drawer: PanelService;` to `Services`, documented as "the shell's right inspector and bottom drawer (design 59, D1078)". Export both types from `src/index.ts`.

In `slots/src/types.ts`:

```ts
/** What an inspector or drawer tab receives (design 59, D1078). */
export interface PanelTabProps {
  /** The current route's params. */
  params: Record<string, string>;
  /** What `inspector.open({ tab, props })` passed for this tab; empty otherwise. */
  props: Readonly<Record<string, unknown>>;
}
// in SlotMap:
  /** A tab of the right inspector; `meta.id` is its tab id, `meta.label` its name, `meta.routes` where it shows. */
  'shell.inspector.tab': { kind: 'list'; props: PanelTabProps };
  /** A tab of the bottom drawer, as `shell.inspector.tab`. */
  'shell.drawer.tab': { kind: 'list'; props: PanelTabProps };
// in SlotMeta:
  /** Route patterns ("/studio/t/:threadId") where an inspector or drawer tab shows; none means everywhere. */
  routes?: readonly string[];
```

Add both slots to `SLOT_KINDS` as `'list'`, and to the shell's and the agent plugin's `loams.plugin.slots`.

- [ ] **Step 4: The controller**

`web/plugins/shell/src/panel.ts`:

```ts
// The inspector and the drawer (design 59, D1078): open or closed, the selected tab, the props
// a caller passed with `open({ tab, props })` (not persisted), and the size, kept per device.

import type { PanelService, PanelState } from '@loams/console-host';

export interface PanelConfig {
  key: string;
  min: number;
  max: number;
  size: number;
}

export const INSPECTOR: PanelConfig = { key: 'loams.shell.inspector', min: 240, max: 640, size: 280 };
export const DRAWER: PanelConfig = { key: 'loams.shell.drawer', min: 160, max: 560, size: 280 };

type Store = Pick<Storage, 'getItem' | 'setItem'>;

function browserStorage(): Store | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

export class PanelController implements PanelService {
  #state: PanelState;
  readonly #listeners = new Set<() => void>();

  constructor(
    private readonly config: PanelConfig,
    private readonly storage: Store | undefined = browserStorage(),
  ) {
    this.#state = this.#read();
  }

  state(): PanelState {
    return this.#state;
  }

  open(o: { tab?: string; props?: Record<string, unknown> } = {}): void {
    const tab = o.tab ?? this.#state.tab;
    const props = o.props ?? (tab === this.#state.tab ? this.#state.props : {});
    this.#set({ open: true, tab, props });
  }

  close(): void {
    this.#set({ open: false });
  }

  toggle(tab?: string): void {
    if (this.#state.open && (tab === undefined || tab === this.#state.tab)) this.close();
    else this.open(tab === undefined ? {} : { tab });
  }

  resize(size: number): void {
    this.#set({ size: this.#clamp(size) });
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  #clamp(n: number): number {
    return Math.min(this.config.max, Math.max(this.config.min, Math.round(n)));
  }

  #read(): PanelState {
    const fallback: PanelState = { open: false, tab: undefined, props: {}, size: this.config.size };
    try {
      const raw = JSON.parse(this.storage?.getItem(this.config.key) ?? 'null') as Partial<PanelState> | null;
      if (!raw || typeof raw !== 'object') return fallback;
      return {
        open: raw.open === true,
        tab: typeof raw.tab === 'string' ? raw.tab : undefined,
        props: {},
        size: typeof raw.size === 'number' ? this.#clamp(raw.size) : this.config.size,
      };
    } catch {
      return fallback;
    }
  }

  #set(patch: Partial<PanelState>): void {
    this.#state = { ...this.#state, ...patch };
    const { open, tab, size } = this.#state;
    try {
      this.storage?.setItem(this.config.key, JSON.stringify({ open, tab, size }));
    } catch {
      // Storage blocked (a private window): the panel still works this session.
    }
    for (const listener of [...this.#listeners]) listener();
  }
}
```

- [ ] **Step 5: The regions**

`web/plugins/shell/src/regions.tsx`:

```tsx
// The right inspector and the bottom drawer (design 59, D1078). Tabs come from
// `shell.inspector.tab` / `shell.drawer.tab`; a tab with `meta.routes` shows only there. A tab
// mounts the first time it is shown and then stays mounted (hidden), so its state survives
// switching tabs and closing the panel (the agent's chat did the same in the old dock).

import type { PanelService } from '@loams/console-host';
import { SlotBoundary, type SlotEntry, useSlot } from '@loams/slots';
import { ResizeHandle, Sheet, Tabs } from '@loams/ui';
import { PanelBottom, PanelRight } from 'lucide-react';
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { useNarrow } from './frame.js';
import { DRAWER, INSPECTOR } from './panel.js';
import { compile, type HashRouter } from './router.js';

type TabSlot = 'shell.inspector.tab' | 'shell.drawer.tab';

export function tabShows(entry: Pick<SlotEntry, 'meta'>, path: string): boolean {
  const routes = entry.meta?.routes;
  if (!routes || routes.length === 0) return true;
  return routes.some((r) => compile(r).pattern.test(path));
}

function usePanel(panel: PanelService) {
  return useSyncExternalStore(panel.subscribe, () => panel.state());
}

function useTabs(slot: TabSlot, router: HashRouter) {
  const entries = useSlot(slot);
  // The old right dock (`shell.dock.right`) is an inspector tab now (W10).
  const dock = useSlot('shell.dock.right');
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  const all: SlotEntry<TabSlot>[] =
    slot === 'shell.inspector.tab'
      ? [
          ...entries,
          ...dock.map((d) => ({ ...d, meta: { id: 'dock', label: d.meta?.label ?? 'Panel' } }) as unknown as SlotEntry<TabSlot>),
        ]
      : entries;
  return { tabs: all.filter((e) => tabShows(e, location.path)), params: location.params };
}

export function PanelRegion({ kind, panel, router }: { kind: 'inspector' | 'drawer'; panel: PanelService; router: HashRouter }) {
  const config = kind === 'inspector' ? INSPECTOR : DRAWER;
  const label = kind === 'inspector' ? 'Inspector' : 'Drawer';
  const state = usePanel(panel);
  const narrow = useNarrow();
  const { tabs, params } = useTabs(kind === 'inspector' ? 'shell.inspector.tab' : 'shell.drawer.tab', router);
  const active = tabs.find((t) => t.meta?.id === state.tab) ?? tabs[0];
  const [mounted, setMounted] = useState<ReadonlySet<string>>(new Set());
  const region = useRef<HTMLElement>(null);
  const activeId = active?.meta?.id ?? String(active?.id ?? '');
  if (state.open && active && !mounted.has(activeId)) setMounted(new Set([...mounted, activeId]));

  useEffect(() => {
    if (!state.open) return;
    const el = region.current;
    (el?.querySelector<HTMLElement>('[data-panel-autofocus], [data-dock-autofocus]') ?? el)?.focus();
  }, [state.open]);

  if (tabs.length === 0 || mounted.size === 0) return null;
  const body = (
    <>
      {tabs.length > 1 && (
        <Tabs
          label={`${label} tabs`}
          value={activeId}
          onValueChange={(tab) => panel.open({ tab })}
          tabs={tabs.map((t) => ({ value: t.meta?.id ?? String(t.id), label: t.meta?.label ?? 'Tab' }))}
        />
      )}
      {tabs.map((t) => {
        const id = t.meta?.id ?? String(t.id);
        if (!mounted.has(id)) return null;
        const Entry = t.component;
        return (
          <div key={t.id} className="lc-panel-tab" hidden={id !== activeId}>
            <SlotBoundary plugin={t.plugin}>
              <Entry params={params} props={id === state.tab ? state.props : {}} />
            </SlotBoundary>
          </div>
        );
      })}
    </>
  );
  if (kind === 'inspector' && narrow) {
    return (
      <Sheet open={state.open} onClose={() => panel.close()} title={label} side="right">
        {body}
      </Sheet>
    );
  }
  const handle = (
    <ResizeHandle
      label={`Resize ${label.toLowerCase()}`}
      value={state.size}
      min={config.min}
      max={config.max}
      orientation={kind === 'inspector' ? 'vertical' : 'horizontal'}
      edge="start"
      onResize={(n) => panel.resize(n)}
    />
  );
  return kind === 'inspector' ? (
    <>
      {state.open && handle}
      <aside ref={region} className="lc-inspector" aria-label={label} tabIndex={-1} hidden={!state.open} style={{ width: state.size }}>
        {body}
      </aside>
    </>
  ) : (
    <>
      {state.open && handle}
      <section ref={region} className="lc-drawer" aria-label={label} tabIndex={-1} hidden={!state.open} style={{ height: state.size }}>
        {body}
      </section>
    </>
  );
}

/** The top bar's inspector and drawer toggles; each shows only while it has a tab here. */
export function PanelToggles({ inspector, drawer, router }: { inspector: PanelService; drawer: PanelService; router: HashRouter }) {
  const i = usePanel(inspector);
  const d = usePanel(drawer);
  const inspectorTabs = useTabs('shell.inspector.tab', router).tabs.length;
  const drawerTabs = useTabs('shell.drawer.tab', router).tabs.length;
  const mac = /mac|iphone|ipad/i.test(globalThis.navigator?.platform ?? '');
  const mod = mac ? '⌘' : 'Ctrl+';
  return (
    <>
      {drawerTabs > 0 && (
        <button
          type="button"
          className="lc-bar-btn"
          aria-pressed={d.open}
          aria-label={d.open ? 'Hide drawer' : 'Show drawer'}
          aria-keyshortcuts="Control+J Meta+J"
          title={`Drawer (${mod}J)`}
          onClick={() => drawer.toggle()}
        >
          <PanelBottom size={16} strokeWidth={1.75} aria-hidden="true" />
        </button>
      )}
      {inspectorTabs > 0 && (
        <button
          type="button"
          className="lc-bar-btn"
          aria-pressed={i.open}
          aria-label={i.open ? 'Hide inspector' : 'Show inspector'}
          aria-keyshortcuts="Control+I Meta+I"
          title={`Inspector (${mod}I)`}
          onClick={() => inspector.toggle()}
        >
          <PanelRight size={16} strokeWidth={1.75} aria-hidden="true" />
        </button>
      )}
    </>
  );
}
```

`Tabs` (Task 5's `tabs.tsx`) renders `role="tablist"` with `role="tab"` buttons. The test finds them by name.

In `layout.tsx`:
1. Delete `DOCK_*`, `readDock`, `useDock`, `DockHandle`, `clampWidth`, the focus effect and the ⌘J `useEffect`.
2. Add `inspector?: PanelService; drawer?: PanelService` to `Layout`'s props, defaulting to `useMemo(() => new PanelController(INSPECTOR), [])` and the same for `DRAWER`.
3. In the work card, render `<PanelRegion kind="inspector" panel={inspector} router={router} />` after `<main>` in `.lc-work-row`, and `<PanelRegion kind="drawer" panel={drawer} router={router} />` after `.lc-work-row`.
4. Pass `trailing={<PanelToggles inspector={inspector} drawer={drawer} router={router} />}` to `TopBar`, in place of Task 7's Agent dock toggle (T7-4).
5. Add a temporary key listener, which Task 12 replaces with the keymap:

```tsx
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
      const k = e.key.toLowerCase();
      if (k === 'i') { e.preventDefault(); inspector.toggle(); }
      else if (k === 'j') { e.preventDefault(); drawer.toggle(); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [inspector, drawer]);
```

6. `Palette` takes `inspector: PanelService` in place of its `dock` prop, and its "Show agent panel" command becomes `{ id: 'agent', title: 'Open the agent', group: 'General', keywords: ['agent', 'assistant', 'inspector'], mode: current, run: () => inspector.open({ tab: 'agent' }) }`.

In `index.tsx`:

```tsx
    const inspector = new PanelController(INSPECTOR);
    const drawer = new PanelController(DRAWER);
    ctx.provide('inspector', inspector);
    ctx.provide('drawer', drawer);
    ctx.effect(() =>
      slots.register({ name: 'root', plugin: 'shell' }, () => (
        <Layout router={router} session={session} flags={flags} mode={mode} theme={theme} inspector={inspector} drawer={drawer} />
      )),
    );
```

Add `"inspector"` and `"drawer"` to the shell manifest's `provides`.

In `web/plugins/agent/src/index.tsx:57`:

```tsx
      slots.register(
        { name: 'shell.inspector.tab', plugin: 'agent', order: 100, meta: { id: 'agent', label: 'Agent' } },
        () => <AgentPanel store={store} desktop={desktop} />,
      ),
```

In `console.css`, the `.lc-dock*` rules become:

```css
.lc-inspector {
  display: flex;
  flex: none;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
  border-left: 1px solid var(--op-rule);
  background: var(--op-surface);
}
.lc-drawer {
  display: flex;
  flex: none;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
  border-top: 1px solid var(--op-rule);
  background: var(--op-surface);
}
.lc-inspector:focus,
.lc-drawer:focus {
  outline: none;
}
.lc-panel-tab {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: auto;
}
.lc-panel-tab[hidden] {
  display: none;
}
```

- [ ] **Step 6: Run the tests**

```bash
pnpm --filter @loams/plugin-shell test
pnpm --filter @loams/plugin-agent test
pnpm --filter @loams/slots test
pnpm -C web typecheck
```

Expected: PASS. The agent panel's own tests do not depend on the slot it registers into.

- [ ] **Step 7: Commit**

```bash
git add web/packages/console-host web/packages/slots web/plugins/shell web/plugins/agent web/apps/console
git commit -s -m "feat(shell): the inspector and drawer regions, inspector.open() and the agent tab (CF1 task 8)"
```

---

### Task 9: Studio in the regions: threads in the sidebar, the home slot, inspector and drawer tabs

**Files:**
- Modify: `web/packages/console-host/src/mode.ts` (`ModeController.prepare`), `src/boot.ts:312-335,436-452` (prepare the other available modes after boot)
- Create: `web/packages/console-host/test/mode-prepare.test.ts`
- Create: `web/plugins/studio/src/regions.tsx` (`registerStudioRegions`, `StudioThreads`, `StudioHome`, `StudioThreadTab`, `StudioTerminalTab`, `StudioTools`, `studioRouteAt`, `STUDIO_TABS`, `THREAD_ROUTES`)
- Modify: `web/plugins/studio/src/index.tsx:130-218` (register the regions; `shell.header.server` → `shell.topbar.left`), `package.json` (`inject` + `inspector`, `drawer`; `slots`)
- Modify: `web/plugins/studio/src/layout/StudioLayout.tsx` (whole file: the main column only)
- Modify: `web/plugins/studio/src/view.tsx` (`ThreadPanel` → `ThreadPanelBody` without its tab strip; `StudioPage` drops `sidebar`/`panel`/`drawer`/`tools`; `?panel=` opens the inspector)
- Modify: `web/plugins/studio/src/sidebar/Sidebar.tsx` (28px rows, the per-project cap, "Show archived" removed)
- Create: `web/plugins/studio/src/settings/Archived.tsx`; modify `src/settings/Settings.tsx` (mount it; `navigate` prop)
- Modify: `web/plugins/studio/src/header.tsx` (registered in `shell.topbar.left`)
- Modify: tests that import `LAYOUT_DEFAULTS` or `LAYOUT_KEY` (`grep -rn "LAYOUT_DEFAULTS\|LAYOUT_KEY" web`)
- Create: `web/plugins/studio/test/shell-sidebar.test.tsx` (Review Focus 2)

**Interfaces:**
- Consumes:
  - From Task 7: the slots `shell.sidebar.threads`, `shell.home` and `shell.topbar.left`/`right`.
  - From Task 8: `shell.inspector.tab`, `shell.drawer.tab`, `PanelService` (`inspector`, `drawer`), `PanelTabProps` and `SlotMeta.routes`.
  - Studio's existing `StudioSidebar`, `Home`, `NotConnected`, `DiffTab`, `FilesTab`, `PreviewTab`, `ThreadTerminalDrawer`, `StudioGitTools`, `StudioContext`, `routeOf`, `STUDIO_PAGES` and `useReach`.
- Produces:
  - `ModeController.prepare(mode: Mode): void`.
  - `registerStudioRegions(slots: SlotRegistry, deps: StudioRegionDeps): (() => void)[]`.
  - `StudioRegionDeps = { agentd: StudioAgentd; factory?: StudioFactory; router: RouterService; keys: StudioKeys; inspector: PanelService; drawer: PanelService; gitBus: GitActionBus; openExternal: (url: string) => Promise<void> }`.
  - `STUDIO_TABS` (diff, files, preview, plan), `THREAD_ROUTES = ['/studio/t/:threadId']` and the tab ids `studio.diff`, `studio.files`, `studio.preview`, `studio.plan` and `studio.terminal`.
  - `THREADS_SHOWN = 10` (Sidebar.tsx).

- [ ] **Step 1: Write the failing tests**

`web/packages/console-host/test/mode-prepare.test.ts`:

```ts
import { describe, expect, it, vi } from 'vitest';
import { ModeController } from '../src/mode.js';

describe('ModeController.prepare (W10)', () => {
  it("loads another mode's plugins once without switching to it", () => {
    const enter = vi.fn();
    const navigate = vi.fn();
    const mode = new ModeController({ current: 'console', available: ['console', 'studio'], modes: new Map(), navigate, enter });
    mode.prepare('studio');
    mode.prepare('studio');
    expect(enter).toHaveBeenCalledOnce();
    expect(mode.current).toBe('console');
    expect(navigate).not.toHaveBeenCalled();
    mode.set('studio');
    expect(enter).toHaveBeenCalledOnce(); // already entered
  });

  it('does nothing for a mode that is not available (the web gate, SD1 Task 15)', () => {
    const enter = vi.fn();
    const mode = new ModeController({ current: 'console', available: ['console'], modes: new Map(), navigate: vi.fn(), enter });
    mode.prepare('studio');
    expect(enter).not.toHaveBeenCalled();
  });
});
```

`web/plugins/studio/test/shell-sidebar.test.tsx`:

```tsx
// Studio in the shell's regions (design 59 D1078, D1079) and Review Focus 2: many threads and
// very long names in the sidebar.

import type { PanelService, RouterService } from '@loams/console-host';
import { SlotRegistry } from '@loams/slots';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createStudioKeys } from '../src/palette/commands.js';
import { GitActionBus } from '../src/git/bus.js';
import { registerStudioRegions, StudioThreads, THREAD_ROUTES } from '../src/regions.js';
import { THREADS_SHOWN } from '../src/sidebar/Sidebar.js';
import { StudioView } from '../src/view.js';
import { webFake, wrap } from './fakes.js';
import { at, sidebarDaemon } from './sidebar-fakes.js';

function router(path = '/data'): RouterService {
  return {
    current: () => ({ path, params: {} }),
    subscribe: () => () => {},
    navigate: vi.fn(),
    page: () => () => {},
  };
}
function panel(): PanelService & { calls: unknown[] } {
  const calls: unknown[] = [];
  const state = { open: false, tab: undefined, props: {}, size: 280 };
  return {
    calls,
    state: () => state,
    open: (o) => void calls.push(['open', o]),
    close: () => void calls.push(['close']),
    toggle: (t) => void calls.push(['toggle', t]),
    resize: () => {},
    subscribe: () => () => {},
  };
}

beforeEach(() => {
  localStorage.clear();
  window.matchMedia = ((q: string) => ({ matches: q.includes('min-width'), media: q, addEventListener() {}, removeEventListener() {} })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the sidebar thread list (Review Focus 2)', () => {
  it('long names truncate and big projects collapse', async () => {
    const long = 'x'.repeat(300);
    const chats = Array.from({ length: 400 }, (_, i) => ({
      id: `t-${i}`,
      title: i === 0 ? `${long} first` : `Thread ${i} ${i % 7 === 0 ? long : ''}`,
      cwd: i < 250 ? '~/src/loams' : i < 380 ? '~/src/acme-site' : '/tmp/elsewhere',
      createdAtMs: at(i),
      lastMessageAtMs: at(1000 - i),
    }));
    const daemon = sidebarDaemon({ chats });
    render(wrap(<StudioThreads agentd={daemon.agentd} router={router()} keys={createStudioKeys()} />));
    const list = await screen.findByRole('list', { name: 'loams threads' });
    const links = within(list)
      .getAllByRole('link')
      .filter((a) => a.getAttribute('href')?.startsWith('#/studio/t/'));
    expect(links).toHaveLength(THREADS_SHOWN);
    const first = links[0] as HTMLElement;
    expect(first.getAttribute('title')).toContain(long);
    expect(first.className).toContain('h-7');
    expect(first.querySelector('.truncate')).toBeTruthy();
    fireEvent.click(within(list).getByRole('button', { name: `Show ${250 - THREADS_SHOWN} more` }));
    await waitFor(() =>
      expect(within(list).getAllByRole('link').filter((a) => a.getAttribute('href')?.startsWith('#/studio/t/'))).toHaveLength(250),
    );
  });

  it('shows nothing without a reachable agentd (Q934)', () => {
    const { agentd } = webFake(); // the web edition with no agentd host
    const { container } = render(wrap(<StudioThreads agentd={agentd} router={router()} keys={createStudioKeys()} />));
    expect(container.textContent).toBe('');
  });

  it('archived threads are not in the sidebar; they live in Settings (D1079)', async () => {
    const daemon = sidebarDaemon({ chats: [{ id: 'old', title: 'Old idea', cwd: '~/src/loams', archived: true, createdAtMs: at(0) }] });
    render(wrap(<StudioThreads agentd={daemon.agentd} router={router()} keys={createStudioKeys()} />));
    await screen.findByRole('navigation', { name: 'Projects' });
    expect(screen.queryByRole('button', { name: /Show archived/ })).toBeNull();
    expect(screen.queryByText('Old idea')).toBeNull();
  });
});

describe('Studio registers its regions', () => {
  it('home in Studio mode, threads, four inspector tabs and the terminal on thread routes', () => {
    const slots = new SlotRegistry();
    const daemon = sidebarDaemon();
    registerStudioRegions(slots, {
      agentd: daemon.agentd,
      router: router(),
      keys: createStudioKeys(),
      inspector: panel(),
      drawer: panel(),
      gitBus: new GitActionBus(),
      openExternal: async () => {},
    });
    expect(slots.entries('shell.home')[0]?.meta?.mode).toBe('studio');
    expect(slots.entries('shell.sidebar.threads')).toHaveLength(1);
    expect(slots.entries('shell.inspector.tab').map((e) => [e.meta?.id, e.meta?.routes])).toEqual([
      ['studio.diff', THREAD_ROUTES],
      ['studio.files', THREAD_ROUTES],
      ['studio.preview', THREAD_ROUTES],
      ['studio.plan', THREAD_ROUTES],
    ]);
    expect(slots.entries('shell.drawer.tab').map((e) => e.meta?.id)).toEqual(['studio.terminal']);
    expect(slots.entries('shell.topbar.left')[0]?.meta?.mode).toBe('studio');
  });

  it('a ?panel= link opens that inspector tab', async () => {
    const daemon = sidebarDaemon();
    const inspector = panel();
    render(
      wrap(
        <StudioView
          agentd={daemon.agentd}
          route={{ kind: 'thread', threadId: 't-login', panel: 'files' }}
          navigate={vi.fn()}
          inspector={inspector}
          drawer={panel()}
        />,
      ),
    );
    await waitFor(() => expect(inspector.calls).toContainEqual(['open', { tab: 'studio.files', props: {} }]));
  });
});
```

- [ ] **Step 2: Run them and watch them fail**

```bash
pnpm --filter @loams/console-host exec vitest run test/mode-prepare.test.ts
pnpm --filter @loams/plugin-studio exec vitest run test/shell-sidebar.test.tsx
```

Expected: FAIL. `mode.prepare is not a function`, and `Cannot find module '../src/regions.js'`.

- [ ] **Step 3: Prepare Studio at boot (W10)**

In `mode.ts`, add to `ModeController`:

```ts
  /**
   * Loads a mode's plugins without switching to it (design 59, W10): Studio's threads show in
   * the shell's sidebar in both modes. An unavailable mode (the web's `studio` gate) is left alone.
   */
  prepare(mode: Mode): void {
    if (!this.o.available.includes(mode) || this.#entered.has(mode)) return;
    this.#entered.add(mode);
    this.o.enter(mode);
  }
```

In `boot.ts`, after the plugin loop (after line 452's loop ends):

```ts
  // Design 59 (W10): the shell's sidebar shows Studio's threads in Console mode too, so every
  // available mode's plugins load at boot, deferred until the boot mode's are applied.
  for (const m of available) if (m !== bootMode) mode.prepare(m);
```

- [ ] **Step 4: Studio's region entries**

`web/plugins/studio/src/regions.tsx`:

```tsx
// Studio in the shell's regions (design 59, D1078–D1079): its projects and threads in the
// sidebar (W6), its home in `shell.home` (W11), its thread panels as inspector tabs and its
// terminal as a drawer tab on thread routes, and its context and git tools in the top bar.

import type { Location, PanelService, RouterService } from '@loams/console-host';
import { AgentdScope } from '@loams/console-host/query';
import type { PanelTabProps, SlotRegistry } from '@loams/slots';
import { useSyncExternalStore } from 'react';
import type { StudioAgentd } from './agentd.js';
import type { StudioFactory } from './factory.js';
import type { GitActionBus } from './git/bus.js';
import { StudioGitTools } from './git/GitToolbar.js';
import { StudioContext } from './header.js';
import { Home } from './home/Home.js';
import { NotConnected } from './home/states.js';
import type { StudioKeys } from './palette/commands.js';
import { useReach } from './projects/queries.js';
import { type Panel, routeOf, STUDIO_PAGES, type StudioRoute } from './routes.js';
import { FactoryGroup } from './sidebar/Factory.js';
import { StudioSidebar } from './sidebar/Sidebar.js';
import { DraftCard } from './thread/FactoryLinks.js';
import { ThreadTerminalDrawer } from './thread/TerminalDrawer.js';
import { ThreadPanelBody } from './view.js';

export const THREAD_ROUTES: readonly string[] = ['/studio/t/:threadId'];
export const STUDIO_TABS: readonly { panel: Exclude<Panel, 'terminal'>; label: string }[] = [
  { panel: 'diff', label: 'Diff' },
  { panel: 'files', label: 'Files' },
  { panel: 'preview', label: 'Preview' },
  { panel: 'plan', label: 'Plan' },
];

export interface StudioRegionDeps {
  agentd: StudioAgentd;
  factory?: StudioFactory;
  router: RouterService;
  keys: StudioKeys;
  inspector: PanelService;
  drawer: PanelService;
  gitBus: GitActionBus;
  openExternal: (url: string) => Promise<void>;
}

/** The Studio route the router is on, or the home for any other page. */
export function studioRouteAt(location: Location): StudioRoute {
  const page = STUDIO_PAGES.find((p) => p.id === location.pageId);
  return page ? routeOf(page.kind, location.params) : { kind: 'home' };
}

function useRoute(router: RouterService): StudioRoute {
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  return studioRouteAt(location);
}

function Threads({ agentd, factory, router, keys }: Pick<StudioRegionDeps, 'agentd' | 'factory' | 'router' | 'keys'>) {
  const reach = useReach(agentd);
  const route = useRoute(router);
  // Q934: the thread list shows when an agentd is reachable; otherwise the sidebar has Pages only.
  if (reach.state !== 'ready') return null;
  return (
    <StudioSidebar agentd={agentd} route={route} navigate={(to) => router.navigate(to)} keys={keys}>
      {factory && <FactoryGroup factory={factory} />}
    </StudioSidebar>
  );
}

export function StudioThreads(props: Pick<StudioRegionDeps, 'agentd' | 'factory' | 'router' | 'keys'>) {
  return (
    <AgentdScope agentd={props.agentd}>
      <Threads {...props} />
    </AgentdScope>
  );
}

function HomeBody({ agentd, params }: { agentd: StudioAgentd; params: Record<string, string> }) {
  const reach = useReach(agentd);
  const route = routeOf('home', params);
  if (reach.state !== 'ready') return <NotConnected agentd={agentd} reach={reach} />;
  return (
    <div className="flex w-full max-w-[720px] flex-col gap-4">
      {route.kind === 'home' && route.draft && <DraftCard draft={route.draft} />}
      <Home agentd={agentd} />
    </div>
  );
}

/** The existing Studio composer, centred (§2: until Part 2 fills the home). */
export function StudioHome({ agentd, params }: { agentd: StudioAgentd; params: Record<string, string> }) {
  return (
    <AgentdScope agentd={agentd}>
      <HomeBody agentd={agentd} params={params} />
    </AgentdScope>
  );
}

export function StudioThreadTab({
  agentd,
  panel,
  params,
  navigate,
  openExternal,
}: { agentd: StudioAgentd; panel: Exclude<Panel, 'terminal'>; navigate: (to: string) => void; openExternal: (url: string) => Promise<void> } & Pick<PanelTabProps, 'params'>) {
  const route = routeOf('thread', params);
  if (route.kind !== 'thread') return null;
  return (
    <AgentdScope agentd={agentd}>
      <ThreadPanelBody agentd={agentd} route={{ ...route, panel }} navigate={navigate} openExternal={openExternal} />
    </AgentdScope>
  );
}

export function StudioTerminalTab({ agentd, keys, params, navigate, openExternal }: Pick<StudioRegionDeps, 'agentd' | 'keys' | 'openExternal'> & { navigate: (to: string) => void } & Pick<PanelTabProps, 'params'>) {
  const threadId = params.threadId;
  if (!threadId) return null;
  return (
    <AgentdScope agentd={agentd}>
      <ThreadTerminalDrawer threadId={threadId} keys={keys} navigate={navigate} openExternal={openExternal} />
    </AgentdScope>
  );
}

function Tools({ agentd, factory, router, gitBus, drawer }: Pick<StudioRegionDeps, 'agentd' | 'factory' | 'router' | 'gitBus' | 'drawer'>) {
  const reach = useReach(agentd);
  const route = useRoute(router);
  if (reach.state !== 'ready' || (route.kind !== 'thread' && route.kind !== 'project')) return null;
  return (
    <StudioGitTools
      agentd={agentd}
      route={route}
      factory={factory}
      bus={gitBus}
      openTerminal={() => drawer.open({ tab: 'studio.terminal' })}
    />
  );
}

export function StudioTools(props: Pick<StudioRegionDeps, 'agentd' | 'factory' | 'router' | 'gitBus' | 'drawer'>) {
  return (
    <AgentdScope agentd={props.agentd}>
      <Tools {...props} />
    </AgentdScope>
  );
}

export function registerStudioRegions(slots: SlotRegistry, d: StudioRegionDeps): (() => void)[] {
  const navigate = (to: string) => d.router.navigate(to);
  return [
    slots.register({ name: 'shell.sidebar.threads', plugin: 'studio' }, () => (
      <StudioThreads agentd={d.agentd} factory={d.factory} router={d.router} keys={d.keys} />
    )),
    slots.register({ name: 'shell.home', plugin: 'studio', meta: { mode: 'studio' } }, ({ params }) => (
      <StudioHome agentd={d.agentd} params={params} />
    )),
    ...STUDIO_TABS.map(({ panel, label }, i) =>
      slots.register(
        { name: 'shell.inspector.tab', plugin: 'studio', order: i, meta: { id: `studio.${panel}`, label, routes: THREAD_ROUTES } },
        ({ params }) => <StudioThreadTab agentd={d.agentd} panel={panel} params={params} navigate={navigate} openExternal={d.openExternal} />,
      ),
    ),
    slots.register(
      { name: 'shell.drawer.tab', plugin: 'studio', meta: { id: 'studio.terminal', label: 'Terminal', routes: THREAD_ROUTES } },
      ({ params }) => <StudioTerminalTab agentd={d.agentd} keys={d.keys} params={params} navigate={navigate} openExternal={d.openExternal} />,
    ),
    slots.register({ name: 'shell.topbar.left', plugin: 'studio', meta: { mode: 'studio' } }, () => (
      <StudioContext agentd={d.agentd} router={d.router} />
    )),
    slots.register({ name: 'shell.topbar.right', plugin: 'studio', meta: { mode: 'studio' } }, () => (
      <StudioTools agentd={d.agentd} factory={d.factory} router={d.router} gitBus={d.gitBus} drawer={d.drawer} />
    )),
  ];
}
```

In `index.tsx`, add `'inspector', 'drawer'` to `inject`. Delete the `shell.header.server` registration, and pass `inspector`/`drawer` to every `StudioView`. Then add:

```tsx
    const inspector = service(ctx, 'inspector');
    const drawer = service(ctx, 'drawer');
    ctx.effect(() => {
      const disposers = registerStudioRegions(slots, {
        agentd,
        factory,
        router,
        keys,
        inspector,
        drawer,
        gitBus,
        openExternal: (url) => platform.openExternal(url),
      });
      return () => {
        for (const dispose of disposers) dispose();
      };
    }, 'studio-regions');
```

Add `"inspector"` and `"drawer"` to `loams.plugin.inject`, and the six slot names to `loams.plugin.slots`.

- [ ] **Step 5: The page loses its own sidebar, panel and drawer**

Replace `StudioLayout.tsx`:

```tsx
// Studio's page column (design 59): the sidebar, the right panel and the terminal drawer are
// the shell's now (the sidebar's thread list, the inspector, the drawer). The layout wires
// Studio's commands to them and gives the page a scrolling column.

import type { PanelService } from '@loams/console-host';
import type { ReactNode } from 'react';
import { StudioCommands, useStudioCommand } from '../palette/commands.js';

const NO_COMMANDS = new StudioCommands();

export function StudioLayout({
  commands,
  inspector,
  drawer,
  children,
}: {
  commands?: StudioCommands;
  inspector?: PanelService;
  drawer?: PanelService;
  children: ReactNode;
}) {
  const bus = commands ?? NO_COMMANDS;
  useStudioCommand(bus, 'rightPanel.toggle', commands && inspector ? () => inspector.toggle() : undefined);
  useStudioCommand(bus, 'rightPanel.show', commands && inspector ? () => inspector.open() : undefined);
  useStudioCommand(bus, 'terminal.toggle', commands && drawer ? () => drawer.toggle('studio.terminal') : undefined);
  return (
    <div className="flex h-full min-h-0 flex-col bg-bg" data-studio-layout>
      <div className="min-h-0 flex-1 overflow-auto">{children}</div>
    </div>
  );
}
```

In `view.tsx`:

1. Rename `ThreadPanel` to `export function ThreadPanelBody`. Delete its `<Tabs …/>` block and wrapper `px-3 pt-1`, keeping the body switch on `route.panel ?? 'diff'`.
2. `StudioPage` takes `inspector?: PanelService; drawer?: PanelService`, and returns `<StudioLayout commands={keys.commands} inspector={inspector} drawer={drawer}>`, wrapping the same `connected ? <Body … /> : <NotConnected … />` children it wraps today (view.tsx:283-297), without the `title`, `sidebar`, `tools`, `panel` and `drawer` props.
3. Add the deep-link effect:

```tsx
  useEffect(() => {
    if (!thread?.panel) return;
    if (thread.panel === 'terminal') drawer?.open({ tab: 'studio.terminal' });
    else inspector?.open({ tab: `studio.${thread.panel}`, props: {} });
  }, [thread?.panel, thread?.file?.path, inspector, drawer]);
```

4. In `useStudioRouteCommands`, the `show(panel)` helper opens the matching tab. It reads the current state from the `inspector` it is given (a new parameter):

```ts
  const show = (panel: Panel) => () => {
    if (!thread) return;
    const s = inspector?.state();
    if (s?.open && s.tab === `studio.${panel}`) inspector?.close();
    else navigate(studioHref({ ...thread, panel }));
  };
```

Delete the `Title` component; the shell's breadcrumbs show the page title. In `Body`, the `settings` case becomes `<SettingsPage agentd={agentd} keys={keys} navigate={navigate} />`.

- [ ] **Step 6: The sidebar's rows, cap and archive**

In `Sidebar.tsx`:
1. `ROW` becomes `'flex h-7 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-small text-muted no-underline outline-none hover:bg-row-hover hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent aria-[current=page]:bg-row-selected aria-[current=page]:text-ink aria-[current=page]:shadow-chip'`.
2. Add `export const THREADS_SHOWN = 10;`. In `ProjectSection`, hold `const [all, setAll] = useState(false)` and cap the unpinned threads, keeping the open thread visible:

```tsx
  const visible = all ? threads : threads.filter((t, i) => i < THREADS_SHOWN || rowProps(t).active);
  const more = threads.length - visible.length;
```

   Render `visible` instead of `threads` in the `SortableContext` (and in `items`). After it, render:

```tsx
              {more > 0 && (
                <li className="list-none">
                  <button type="button" className={cx(ROW, 'w-full text-faint')} onClick={() => setAll(true)}>
                    Show {more} more
                  </button>
                </li>
              )}
```

3. Delete `showArchived` and the "Show archived (n)" button (lines 1018–1028), and pass `showArchived: false` to `buildSidebarModel`.

`web/plugins/studio/src/settings/Archived.tsx`:

```tsx
// Archived threads (design 59, D1079): out of the sidebar, in Studio's settings.

import { Button, Empty } from '@loams/ui';
import type { StudioAgentd } from '../agentd.js';
import { useProjects, useReach } from '../projects/queries.js';
import { useSidebarData } from '../sidebar/data.js';
import { useThreadActions } from '../sidebar/useThreadActions.js';

export function ArchivedThreads({ agentd, navigate }: { agentd: StudioAgentd; navigate: (to: string) => void }) {
  const reach = useReach(agentd);
  const projects = useProjects(agentd);
  const [data, applyPins] = useSidebarData(agentd, reach.state === 'ready' ? projects.data : undefined);
  const actions = useThreadActions({ navigate, threads: data.threads, applyPins });
  const archived = data.threads.filter((t) => t.archived);
  return (
    <section aria-labelledby="studio-archived" className="grid gap-2">
      <h2 id="studio-archived" className="m-0 text-section font-semibold">
        Archived threads
      </h2>
      {archived.length === 0 ? (
        <Empty title="No archived threads">Archive a thread from its menu in the sidebar.</Empty>
      ) : (
        <ul className="m-0 grid list-none gap-1 p-0">
          {archived.map((t) => (
            <li key={t.id} className="flex min-w-0 items-center gap-2">
              <a href={`#/studio/t/${t.id}`} className="min-w-0 flex-1 truncate text-body text-ink" title={t.title}>
                {t.title}
              </a>
              <Button size="sm" onClick={() => void actions.setArchived(t.id, false)}>
                Unarchive
              </Button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
```

Mount it at the end of `SettingsPage`. `SettingsPage` takes `navigate`, and `view.tsx` passes it.

- [ ] **Step 7: Run the tests**

```bash
pnpm --filter @loams/console-host test
pnpm --filter @loams/plugin-studio test
pnpm --filter @loams/plugin-shell test
pnpm -C web typecheck
```

Expected: PASS. Studio's existing `sidebar.test.tsx` renders `StudioView`, which no longer contains the sidebar. Change its `show()` helper to render `<StudioThreads agentd={daemon.agentd} router={router(path)} keys={createStudioKeys()} />` beside the view, and keep every assertion. Tests that clicked "Show archived" move to `ArchivedThreads`.

- [ ] **Step 8: Run Studio's end-to-end suites**

```bash
cargo build -p loams-agentd --locked   # the debug daemon the web e2e fixture starts (light: one crate)
pnpm --filter @loams/console exec playwright install chromium
pnpm --filter @loams/console test:e2e
```

Expected: the existing `studio.spec.ts` scenarios pass. Where a spec clicked Studio's own panel toggle or sidebar, change it to the shell's (`Show inspector`, `Show drawer`, the `Sidebar` navigation). Record each changed locator as a `T9-*` ruling.

- [ ] **Step 9: Commit**

```bash
git add web/packages/console-host web/plugins/studio web/apps/console/test
git commit -s -m "feat(studio): threads in the shell sidebar, the home slot, inspector and drawer tabs (CF1 task 9)"
```

---

### Task 10: Navigation in three places: pinned Pages, the palette, Settings

**Files:**
- Create: `web/plugins/shell/src/pages.ts`
- Modify: `web/plugins/shell/src/sidebar.tsx` (`Nav` and its section headings are replaced by `PagesSection` and `SettingsNav`; `EnvironmentSwitcher` moves to the top bar)
- Modify: `web/plugins/shell/src/frame.tsx` (`Crumbs` without sections; the branch pill), `src/layout.tsx` (the palette's "Go to" rows from `pageEntries`)
- Modify: `web/plugins/projects/src/index.tsx:73-77` (no `shell.nav.context`; the project pill and three pages), `src/context.tsx` (becomes `ProjectPill`), `package.json` (`slots`)
- Modify: `web/apps/console/src/cordis/console.css` (`.lc-nav-heading` and `[data-scope="org"]` rules deleted; `.lc-pages`, `.lc-settings-nav`)
- Create: `web/plugins/shell/test/pages.test.tsx`
- Modify: `web/plugins/shell/test/layout.test.tsx` (the section-grouping tests are deleted; `pages.test.tsx` replaces them), `web/plugins/projects/test/projects.test.tsx`

**Interfaces:**
- Consumes: `shell.nav.section` and `console.nav` entries (unchanged); `Menu` and `MenuItem` (Task 5); `router.page` routes (unchanged).
- Produces: `DEFAULT_PINNED`, `SETTINGS_GROUPS`, `PageEntry`, `pageEntries`, `pinnedPages`, `readPinned`, `writePinned`, `isSettingsPage`, `currentProject`, and `settingsHome(all: readonly PageEntry[]): PageEntry | undefined` (used by ⌘, in Task 12).

The page list as of 2026-10-17, and where each page lands:

| Page (`meta.id`) | Route | Today's section | Lands in |
|---|---|---|---|
| overview | `/` | Data | Pages (palette; pinnable) |
| canvas, data, sql, postgres, wesql, live, analytics | `/canvas`, `/data`, `/sql`, `/postgres`, `/wesql`, `/live`, `/analytics` | Data | Pages; **data** is pinned by default |
| deployments, durable, streams, jobs | `/deployments`, `/durable`, `/streams`, `/jobs` | Compute | Pages; **deployments** pinned |
| connectors, pipelines, graph, git | `/connectors`, `/pipelines`, `/graph`, `/git` | Integrate | Pages; **connectors** and **git** pinned |
| agents (projects plugin, W20) | `/projects/:project/agents` | (project context) | Pages; **agents** pinned |
| project-overview (projects plugin) | `/projects/:project` | (project context) | Pages |
| access (projects plugin) | `/projects/:project/access` | (project context) | Settings |
| org pages: teams, members, org-settings, org-home | `/teams`, `/members`, … | Organisation | Settings |
| audit | `/audit` | Organisation | Settings |
| approvals (legacy) | `/approvals` | Operate → Organisation | Settings |
| metrics, status (legacy) | `/metrics`, `/status` | Operate | Settings |
| namespaces (legacy) | `/namespaces` | Instance → Data | Pages |
| settings (desktop), studio-settings | `/settings`, `/studio/settings` | Organisation, Studio | Settings |
| factory | `/studio/factory…` | Factory | Pages |
| studio-home | `/studio` | Studio | not listed (the brand and New thread go there) |

- [ ] **Step 1: Write the failing tests**

`web/plugins/shell/test/pages.test.tsx`:

```tsx
import { type SessionService, ThemeController } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Layout } from '../src/layout.js';
import { DEFAULT_PINNED, isSettingsPage, pageEntries, pinnedPages, readPinned, writePinned } from '../src/pages.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  const ls = new Set<() => void>();
  return {
    get: () => path,
    set: (p) => {
      path = p;
      for (const l of ls) l();
    },
    listen: (l) => {
      ls.add(l);
      return () => ls.delete(l);
    },
  };
}
const session = { principal: () => null, subscribe: () => () => {} } as unknown as SessionService;
const theme = new ThemeController({ read: () => 'dark', write: () => {} });

/** The console's real page set, as the plugins register it. */
const PAGES: [id: string, label: string, href: string, group: string][] = [
  ['overview', 'Overview', '/', 'Data'],
  ['data', 'Data explorer', '/data', 'Data'],
  ['sql', 'SQL', '/sql', 'Data'],
  ['deployments', 'Deployments', '/deployments', 'Compute'],
  ['jobs', 'Jobs', '/jobs', 'Compute'],
  ['connectors', 'Connectors', '/connectors', 'Integrate'],
  ['git', 'Git', '/git', 'Integrate'],
  ['agents', 'Agents', '/projects/:project/agents', 'Project'],
  ['access', 'Access', '/projects/:project/access', 'Organisation'],
  ['teams', 'Teams', '/teams', 'Organisation'],
  ['audit', 'Audit log', '/audit', 'Organisation'],
  ['metrics', 'Metrics', '/metrics', 'Operate'],
];

function setup(start = '/') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  PAGES.forEach(([id, label, href, group], order) => {
    slots.register({ name: 'shell.nav.section', plugin: id, order, meta: { id, label, href, group } }, () => null);
    router.page({ id: `page-${id}`, path: href, title: label, plugin: id }, () => <p>{label} page</p>);
  });
  render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} theme={theme} />
    </SlotProvider>,
  );
  return { slots, router };
}

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('pages (D1080)', () => {
  it('fills :project, hides project pages without a project, and classifies Settings', () => {
    const sections = PAGES.map(([id, label, href, group], order) => ({ id: order, order, plugin: id, name: 'shell.nav.section' as const, meta: { id, label, href, group }, component: () => null }));
    const none = pageEntries(sections, [], undefined);
    expect(none.find((p) => p.id === 'agents')).toBeUndefined();
    const acme = pageEntries(sections, [], 'acme');
    expect(acme.find((p) => p.id === 'agents')?.href).toBe('/projects/acme/agents');
    expect(acme.filter(isSettingsPage).map((p) => p.id).sort()).toEqual(['access', 'audit', 'metrics', 'teams']);
    expect(pinnedPages(acme, DEFAULT_PINNED).map((p) => p.label)).toEqual(['Data explorer', 'Deployments', 'Git', 'Agents', 'Connectors']);
  });

  it('keeps pins per project, and falls back to the defaults (Q932)', () => {
    expect(readPinned('acme')).toEqual([...DEFAULT_PINNED]);
    writePinned('acme', ['sql']);
    expect(readPinned('acme')).toEqual(['sql']);
    expect(readPinned('other')).toEqual([...DEFAULT_PINNED]);
    localStorage.setItem('loams.shell.pages.broken', '{');
    expect(readPinned('broken')).toEqual([...DEFAULT_PINNED]);
  });

  it('the sidebar lists the pinned Pages and no section headings', () => {
    setup('/projects/acme');
    const sidebar = screen.getByRole('navigation', { name: 'Sidebar' });
    const pages = within(sidebar).getByRole('list', { name: 'Pages' });
    expect(within(pages).getAllByRole('link').map((a) => a.textContent)).toEqual([
      'Data explorer',
      'Deployments',
      'Git',
      'Agents',
      'Connectors',
    ]);
    for (const heading of ['Data', 'Compute', 'Organisation', 'Operate', 'Integrate']) {
      expect(within(sidebar).queryByRole('heading', { name: heading })).toBeNull();
    }
  });

  it('pins and unpins a page from the sidebar', async () => {
    setup('/projects/acme');
    fireEvent.click(screen.getByRole('button', { name: 'Edit pages' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: /SQL/ }));
    expect(readPinned('acme')).toContain('sql');
    expect(within(screen.getByRole('list', { name: 'Pages' })).getByRole('link', { name: 'SQL' })).toBeTruthy();
  });

  it('Settings holds organisation admin, with a way back', () => {
    const { router } = setup('/data');
    // Links are plain hash links; the test's router reads a memory source, so it navigates itself.
    expect(screen.getByRole('link', { name: 'Settings' }).getAttribute('href')).toBe('#/teams');
    act(() => router.navigate('/teams'));
    const settings = screen.getByRole('navigation', { name: 'Settings' });
    expect(within(settings).getAllByRole('link').map((a) => a.textContent)).toEqual(
      expect.arrayContaining(['Teams', 'Audit log', 'Metrics']),
    );
    expect(within(settings).getByRole('link', { name: /Back/ }).getAttribute('href')).toBe('#/data');
  });

  it('every page keeps its route and is in the palette', async () => {
    const { router } = setup('/');
    for (const [, label, href] of PAGES) {
      if (href.includes(':project')) continue;
      act(() => router.navigate(href));
      expect(router.current().pageId).toBe(`page-${PAGES.find((p) => p[2] === href)?.[0]}`);
      expect(screen.getByText(`${label} page`)).toBeTruthy();
    }
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    fireEvent.change(await screen.findByRole('combobox'), { target: { value: 'metrics' } });
    expect(await screen.findByRole('option', { name: /Go to Metrics/ })).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run them and watch them fail**

Run: `pnpm --filter @loams/plugin-shell exec vitest run test/pages.test.tsx`
Expected: FAIL, with `Cannot find module '../src/pages.js'`.

- [ ] **Step 3: The page model**

`web/plugins/shell/src/pages.ts`:

```ts
// The console's pages after design 59 (D1080): no section headings. A project pins the few it
// uses daily (Q932's defaults), the palette reaches every page, and the organisation admin
// pages are Settings. Routes do not change.

import type { SlotEntry } from '@loams/slots';

export const DEFAULT_PINNED: readonly string[] = ['data', 'deployments', 'git', 'agents', 'connectors'];
/** The old sections whose pages are Settings now. */
export const SETTINGS_GROUPS: readonly string[] = ['Organisation', 'Operate', 'Instance', 'Settings'];
const SETTINGS_IDS = new Set(['settings', 'studio-settings']);
/** Not listed: the brand and New thread go to Studio's home. */
const HIDDEN_IDS = new Set(['studio-home']);
export const PINNED_KEY = 'loams.shell.pages';
/** The projects plugin's LAST_PROJECT_KEY (plugins/projects/src/context.tsx). */
export const PROJECT_KEY = 'loams-project';
const NO_PROJECT = '_';

export interface PageEntry {
  id: string;
  label: string;
  href: string;
  icon?: string;
  group: string;
  order: number;
}

/** Pages registered through `router.page(… nav)` before `shell.nav.section`, by path. */
const LEGACY: Record<string, Pick<PageEntry, 'id' | 'group' | 'icon' | 'order'>> = {
  '/': { id: 'overview', group: 'Data', icon: 'overview', order: 0 },
  '/status': { id: 'status', group: 'Operate', icon: 'overview', order: 10 },
  '/namespaces': { id: 'namespaces', group: 'Data', icon: 'namespaces', order: 15 },
  '/approvals': { id: 'approvals', group: 'Organisation', icon: 'approvals', order: 50 },
};

type Store = Pick<Storage, 'getItem' | 'setItem'>;
function browser(): Store | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function fill(href: string, project: string | undefined): string | undefined {
  if (!href.includes(':project')) return href;
  return project ? href.replace(':project', encodeURIComponent(project)) : undefined;
}

export function pageEntries(
  sections: readonly SlotEntry<'shell.nav.section'>[],
  legacy: readonly SlotEntry<'console.nav'>[],
  project: string | undefined,
): PageEntry[] {
  const byHref = new Map<string, PageEntry>();
  for (const e of sections) {
    const id = e.meta?.id ?? String(e.id);
    const href = fill(e.meta?.href ?? '/', project);
    if (!href || HIDDEN_IDS.has(id) || byHref.has(href)) continue;
    byHref.set(href, { id, label: e.meta?.label ?? id, href, icon: e.meta?.icon, group: e.meta?.group ?? 'Organisation', order: e.order });
  }
  for (const e of legacy) {
    const raw = e.meta?.href ?? '/';
    const href = fill(raw, project);
    if (!href || byHref.has(href)) continue;
    const known = LEGACY[raw];
    byHref.set(href, {
      id: known?.id ?? `legacy:${raw}`,
      label: e.meta?.label ?? raw,
      href,
      icon: known?.icon ?? (raw.startsWith('/plugins/') ? 'plugin' : undefined),
      group: known?.group ?? e.meta?.group ?? 'Organisation',
      order: known?.order ?? 1000 + e.order,
    });
  }
  return [...byHref.values()].sort((a, b) => a.order - b.order || a.label.localeCompare(b.label));
}

export function isSettingsPage(entry: Pick<PageEntry, 'group' | 'id'>): boolean {
  return SETTINGS_GROUPS.includes(entry.group) || SETTINGS_IDS.has(entry.id);
}

export function pinnedPages(all: readonly PageEntry[], pinned: readonly string[]): PageEntry[] {
  const byId = new Map(all.map((p) => [p.id, p]));
  return pinned.flatMap((id) => {
    const p = byId.get(id);
    return p && !isSettingsPage(p) ? [p] : [];
  });
}

/** Where Settings (and ⌘,) opens: the desktop's settings, else the organisation's, else Studio's. */
export function settingsHome(all: readonly PageEntry[]): PageEntry | undefined {
  const settings = all.filter(isSettingsPage);
  for (const id of ['settings', 'teams', 'org-settings', 'studio-settings']) {
    const hit = settings.find((p) => p.id === id);
    if (hit) return hit;
  }
  return settings[0];
}

export function readPinned(project: string | undefined, storage: Store | undefined = browser()): string[] {
  try {
    const raw = JSON.parse(storage?.getItem(`${PINNED_KEY}.${project ?? NO_PROJECT}`) ?? 'null') as unknown;
    return Array.isArray(raw) && raw.every((x) => typeof x === 'string') ? raw : [...DEFAULT_PINNED];
  } catch {
    return [...DEFAULT_PINNED];
  }
}

export function writePinned(project: string | undefined, ids: readonly string[], storage: Store | undefined = browser()): void {
  try {
    storage?.setItem(`${PINNED_KEY}.${project ?? NO_PROJECT}`, JSON.stringify(ids));
  } catch {
    // Storage blocked: the pins hold for this page.
  }
}

/** The project a route is in (`/projects/<slug>/…`), else the last one opened. */
export function currentProject(path: string, storage: Store | undefined = browser()): string | undefined {
  const m = /^\/projects\/([^/]+)/.exec(path);
  if (m?.[1]) {
    try {
      return decodeURIComponent(m[1]);
    } catch {
      return undefined;
    }
  }
  try {
    return storage?.getItem(PROJECT_KEY) ?? undefined;
  } catch {
    return undefined;
  }
}
```

In this test's page set, `settingsHome` picks `teams`. The `settings` id is absent outside the desktop.

- [ ] **Step 4: The sidebar's Pages and Settings**

In `sidebar.tsx`, replace `<Nav …/>` with `<Navigation router={router} />`:

```tsx
function usePages(router: HashRouter) {
  const sections = useSlot('shell.nav.section');
  const legacy = useSlot('console.nav');
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  const project = currentProject(location.path);
  const all = pageEntries(sections, legacy, project);
  return { all, project, location };
}

function Navigation({ router }: { router: HashRouter }) {
  const { all, project, location } = usePages(router);
  const here = all
    .filter((p) => location.path === p.href || (p.href !== '/' && location.path.startsWith(`${p.href}/`)))
    .sort((a, b) => b.href.length - a.href.length)[0];
  const back = useRef('/');
  if (here && !isSettingsPage(here)) back.current = location.path;
  return here && isSettingsPage(here) ? (
    <SettingsNav pages={all.filter(isSettingsPage)} current={here.id} back={back.current} />
  ) : (
    <PagesSection all={all} project={project} current={here?.id} />
  );
}

function PageLink({ page, current }: { page: PageEntry; current: boolean }) {
  const Icon = iconFor(page.icon);
  return (
    <li>
      <a className="lc-page-link" href={`#${page.href}`} aria-current={current ? 'page' : undefined} title={page.label}>
        <Icon aria-hidden="true" size={15} strokeWidth={1.75} />
        <span className="truncate">{page.label}</span>
      </a>
    </li>
  );
}

function PagesSection({ all, project, current }: { all: PageEntry[]; project: string | undefined; current?: string }) {
  const [pinned, setPinned] = useState(() => readPinned(project));
  useEffect(() => setPinned(readPinned(project)), [project]);
  const toggle = (id: string) => {
    const next = pinned.includes(id) ? pinned.filter((x) => x !== id) : [...pinned, id];
    writePinned(project, next);
    setPinned(next);
  };
  const shown = pinnedPages(all, pinned);
  return (
    <section className="lc-pages" aria-labelledby="lc-pages-title">
      <div className="lc-pages-head">
        <span id="lc-pages-title">Pages</span>
        <Menu
          label="Edit pages"
          trigger={
            <button type="button" className="lc-icon-btn" aria-label="Edit pages">
              <Pin size={13} aria-hidden="true" />
            </button>
          }
        >
          {all.filter((p) => !isSettingsPage(p)).map((p) => (
            <MenuItem key={p.id} onSelect={() => toggle(p.id)}>
              <Check size={14} aria-hidden="true" style={{ visibility: pinned.includes(p.id) ? 'visible' : 'hidden' }} />
              {p.label}
            </MenuItem>
          ))}
        </Menu>
      </div>
      <ul aria-label="Pages">
        {shown.map((p) => (
          <PageLink key={p.id} page={p} current={p.id === current} />
        ))}
      </ul>
    </section>
  );
}

function SettingsNav({ pages, current, back }: { pages: PageEntry[]; current: string; back: string }) {
  return (
    <nav aria-label="Settings" className="lc-settings-nav">
      <a className="lc-page-link" href={`#${back}`}>
        <ArrowLeft size={15} aria-hidden="true" /> Back
      </a>
      <ul>
        {pages.map((p) => (
          <PageLink key={p.id} page={p} current={p.id === current} />
        ))}
      </ul>
    </nav>
  );
}
```

`Pin`, `Check` and `ArrowLeft` come from `lucide-react`; `iconFor` from `./icons.js`. In `SidebarFooter`, put a Settings link before Documentation:

```tsx
        <SettingsLink router={router} />
```

```tsx
function SettingsLink({ router }: { router: HashRouter }) {
  const { all } = usePages(router);
  const home = settingsHome(all);
  if (!home) return null;
  return (
    <a className="lc-page-link" href={`#${home.href}`} aria-keyshortcuts="Control+Comma Meta+Comma">
      <Settings size={15} aria-hidden="true" /> Settings
    </a>
  );
}
```

Delete `Nav` and its CSS (`.lc-nav-heading`, `.lc-nav-group[data-scope="org"]`). Keep `navSections` and `SECTIONS` exported from `nav.ts` for any external caller, and mark them `@deprecated design 59: use pageEntries`. In `frame.tsx`, `Crumbs` loses the section crumb. Its crumbs are the instance (or `project`) and then the page label. The router's `trail()` still wins when a page sets one. In `TopBar`'s start cluster, render the branch pill (W21) in Console mode:

```tsx
        {(mode?.current ?? 'console') === 'console' && <EnvironmentSwitcher session={session} />}
```

`TopBar` takes `session`. `EnvironmentSwitcher` moves from `sidebar.tsx` to `frame.tsx`, and its trigger gets `size="compact"`.

In `console.css`:

```css
.lc-pages,
.lc-settings-nav {
  display: grid;
  gap: 2px;
  padding: 8px;
}
.lc-pages-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 8px;
  color: var(--op-faint);
  font-size: var(--op-text-micro);
  font-weight: 500;
}
.lc-pages ul,
.lc-settings-nav ul {
  display: grid;
  gap: 1px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.lc-page-link {
  display: flex;
  align-items: center;
  gap: 8px;
  height: var(--op-row);
  min-width: 0;
  padding: 0 8px;
  border-radius: var(--op-radius-md);
  color: var(--op-muted);
  font-size: var(--op-text-small);
  font-weight: 500;
  text-decoration: none;
}
.lc-page-link:hover {
  background: var(--op-row-hover);
  color: var(--op-ink);
}
.lc-page-link[aria-current="page"] {
  background: var(--op-row-selected);
  box-shadow: var(--op-shadow-chip);
  color: var(--op-ink);
}
```

- [ ] **Step 5: The palette reaches every page**

In `layout.tsx`'s `Palette`, build the "Go to" rows from `pageEntries` in every mode. Settings pages go under "Settings", the rest under "Pages", and each page gets a pin or unpin command for the current project:

```tsx
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  const project = currentProject(location.path);
  const all = pageEntries(sections, legacy, project);
  const pinned = readPinned(project);
  const nav = (m: Mode): ScopedCommand[] =>
    m !== current
      ? []
      : all.flatMap((p) => [
          {
            id: `go:${p.href}`,
            title: `Go to ${p.label}`,
            group: isSettingsPage(p) ? 'Settings' : 'Pages',
            keywords: [p.group, p.href, p.id],
            mode: m,
            run: () => router.navigate(p.href),
          },
          ...(isSettingsPage(p)
            ? []
            : [
                {
                  id: `pin:${p.id}`,
                  title: pinned.includes(p.id) ? `Unpin ${p.label} from the sidebar` : `Pin ${p.label} to the sidebar`,
                  group: 'Pages',
                  keywords: ['pin', 'sidebar', p.label],
                  secondary: true,
                  mode: m,
                  run: () => writePinned(project, pinned.includes(p.id) ? pinned.filter((x) => x !== p.id) : [...pinned, p.id]),
                },
              ]),
        ]);
```

A pin made from the palette shows in the sidebar the next time `PagesSection` reads the pins: on a project change or a remount. To make it live, `writePinned` dispatches a `loams:pages-changed` event on `window`, and `PagesSection` listens for it and re-reads the pins:

```ts
// in writePinned, after setItem:
  globalThis.dispatchEvent?.(new Event('loams:pages-changed'));
```

```tsx
// in PagesSection:
  useEffect(() => {
    const onChange = () => setPinned(readPinned(project));
    window.addEventListener('loams:pages-changed', onChange);
    return () => window.removeEventListener('loams:pages-changed', onChange);
  }, [project]);
```

- [ ] **Step 6: The projects plugin: the pill and three pages (W20)**

In `web/plugins/projects/src/context.tsx`, `ProjectContext` becomes `ProjectPill`. It is the existing `SelectMenu` switcher, with `size` compact, inside `<div className="lc-project-pill">`, and it drops the `<nav>` of project links. In `index.tsx`:

```tsx
    ctx.effect(() =>
      slots.register({ name: 'shell.topbar.left', plugin: 'projects', order: 0, meta: { mode: 'console' } }, () => (
        <ProjectPill router={router} />
      )),
    );
    const page = (id: string, label: string, href: string, icon: string, group: string, order: number) =>
      ctx.effect(() =>
        slots.register({ name: 'shell.nav.section', plugin: 'projects', order, meta: { id, label, href, icon, group } }, () => null),
      );
    page('project-overview', 'Project overview', '/projects/:project', 'overview', 'Project', 1);
    page('agents', 'Agents', '/projects/:project/agents', 'agents', 'Project', 2);
    page('access', 'Access', '/projects/:project/access', 'access', 'Organisation', 3);
```

Replace `shell.nav.context` with `shell.topbar.left` and `shell.nav.section` in its `package.json` slots. Update `projects.test.tsx`: the switcher tests find the pill in `shell.topbar.left`, and the deleted project-links tests become `pages.test.tsx`'s `:project` case.

- [ ] **Step 7: Run the tests**

```bash
pnpm --filter @loams/plugin-shell test
pnpm --filter @loams/plugin-projects test
pnpm -C web -r --if-present test
pnpm -C web typecheck
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add web/plugins/shell web/plugins/projects web/apps/console
git commit -s -m "feat(shell): pinned Pages per project, every page in the palette, admin in Settings (CF1 task 10)"
```

---

### Task 11: The palette's sources: projects, branches, threads, pages, commands, tables, thread text

**Files:**
- Modify: `web/packages/slots/src/types.ts`, `src/registry.ts` (the `shell.palette.source` slot, W9)
- Modify: `web/plugins/shell/src/palette.tsx:155-160` (read both slots), `package.json` (`slots`)
- Modify: `web/plugins/projects/src/index.tsx`, create `web/plugins/projects/src/palette.tsx` (`ProjectsSource`)
- Modify: `web/plugins/data-studio/src/index.tsx`, create `web/plugins/data-studio/src/palette.tsx` (`TablesSource`)
- Modify: `web/packages/studio-ui/src/palette/logic.ts:32-40,86-90` (`PaletteThread.preview` matched last), `web/plugins/studio/src/sidebar/paletteSource.ts:72-80` (fill `preview`)
- Create: `web/plugins/studio/src/palette/BranchesSource.tsx`; modify `web/plugins/studio/src/index.tsx`
- Modify: `web/plugins/shell/test/palette.test.tsx`, `web/plugins/projects/test/projects.test.tsx`, `web/plugins/data-studio/test/data.test.tsx`, `web/plugins/studio/test/palette.test.tsx`

**Interfaces:**
- Consumes: `PaletteCommand` (`@loams/slots`); Task 10's "Go to" page rows and pin commands; Studio's palette (`palette.command`) for threads, projects, run groups and git actions.
- Produces:
  - The slot `shell.palette.source` (props `{ query; publish }`).
  - `ProjectsSource({ query, publish, router })`, `TablesSource({ query, publish, client, namespace, navigate })` and `BranchesSource({ query, publish, agentd, navigate })`.
  - `PaletteThread.preview?: string`.

- [ ] **Step 1: Write the failing tests**

In `web/plugins/shell/test/palette.test.tsx`, add:

```tsx
  it('reads shell.palette.source as well as palette.command (W9)', async () => {
    const { slots } = setup('console');
    act(() => {
      slots.register({ name: 'shell.palette.source', plugin: 'pages' }, ({ publish }) => {
        useEffect(() => publish([{ id: 'p', title: 'Go to project acme', group: 'Projects', run: () => {} }]), [publish]);
        return null;
      });
    });
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    expect(await screen.findByRole('option', { name: /Go to project acme/ })).toBeTruthy();
  });
```

`web/plugins/projects/test/projects.test.tsx` adds a test of `ProjectsSource` against the file's admin mock (`withConsole`, `stubRouter` from `@loams/admin-ui/testing`, whose mock serves the "Docs search" project):

```tsx
  it('the palette lists every project', async () => {
    const published: PaletteCommand[][] = [];
    const router = stubRouter('/');
    render(withConsole(<ProjectsSource query="" publish={(c) => published.push([...c])} router={router} />));
    await waitFor(() => expect(published.at(-1)?.map((c) => c.title)).toContain('Go to project Docs search'));
    published.at(-1)?.find((c) => c.title === 'Go to project Docs search')?.run();
    expect(router.current().path).toBe('/projects/docs-search');
  });
```

`web/plugins/data-studio/test/data.test.tsx`:

```tsx
  it('the palette lists the namespace\'s tables (D1078)', async () => {
    const published: PaletteCommand[][] = [];
    const navigate = vi.fn();
    const client = { listCollections: vi.fn(async () => [{ name: 'orders' }, { name: 'users' }]) } as unknown as DataClient;
    render(<TablesSource query="ord" publish={(c) => published.push([...c])} client={client} namespace="shop" navigate={navigate} />);
    await waitFor(() => expect(published.at(-1)?.map((c) => c.title)).toEqual(['orders', 'users']));
    published.at(-1)?.[0]?.run();
    expect(navigate).toHaveBeenCalledWith('/data/shop/orders');
  });
```

`web/plugins/studio/test/palette.test.tsx`:

```tsx
  it('matches a thread by its last message (thread text, W13)', () => {
    const items = buildThreadActionItems({
      threads: [
        { id: 't1', title: 'Untitled', projectId: '', updatedAt: 1, preview: 'migrate the orders table to v2' },
        { id: 't2', title: 'Orders page copy', projectId: '', updatedAt: 2 },
      ],
      projectTitleById: new Map(),
      open: () => {},
    });
    const ranked = rankCommands(items.map((i) => ({ ...i, text: i.title })), 'migrate the orders');
    expect(ranked[0]?.id).toBe('thread:t1');
  });
```

`buildThreadActionItems` and `rankCommands` come from `@loams/studio-ui` and `@loams/ui`.

- [ ] **Step 2: Run them and watch them fail**

```bash
pnpm --filter @loams/plugin-shell exec vitest run test/palette.test.tsx
pnpm --filter @loams/plugin-projects test
pnpm --filter @loams/plugin-data-studio test
pnpm --filter @loams/plugin-studio exec vitest run test/palette.test.tsx
```

Expected: FAIL, with `unknown slot shell.palette.source`, `ProjectsSource is not exported`, `TablesSource is not exported`, and a ranking that misses `thread:t1`.

- [ ] **Step 3: The slot and the shell**

In `types.ts`:

```ts
  /**
   * A palette source (design 59, D1078; the same contract as `palette.command`, which stays
   * an alias, W9): mounted with the query while the palette is open, it calls `publish`.
   */
  'shell.palette.source': {
    kind: 'list';
    props: { query: string; publish: (commands: readonly PaletteCommand[]) => void };
  };
```

Add `'shell.palette.source': 'list'` to `SLOT_KINDS`. In `palette.tsx`:

```tsx
  const legacy = useSlot('palette.command');
  const sources = useSlot('shell.palette.source');
  const entries = [...legacy, ...(sources as unknown as SlotEntry<'palette.command'>[])];
```

The two slots have identical props, so the cast is sound. `Source` keeps its `entry: SlotEntry<'palette.command'>` type.

- [ ] **Step 4: The sources**

`web/plugins/projects/src/palette.tsx`:

```tsx
import { useQuery } from '@connectrpc/connect-query';
import type { RouterService } from '@loams/console-host';
import { admin } from '@loams/proto';
import type { PaletteCommand } from '@loams/slots';
import { useEffect } from 'react';

export function ProjectsSource({
  publish,
  router,
}: {
  query: string;
  publish: (commands: readonly PaletteCommand[]) => void;
  router: RouterService;
}) {
  const projects = useQuery(admin.ProjectService.method.listProjects, {});
  const list = projects.data?.projects ?? [];
  useEffect(() => {
    publish(
      list.map((p) => ({
        id: `project:${p.slug}`,
        title: `Go to project ${p.name}`,
        group: 'Projects',
        keywords: [p.slug],
        run: () => router.navigate(`/projects/${encodeURIComponent(p.slug)}`),
      })),
    );
  }, [list, publish, router]);
  return null;
}
```

Register it in `index.tsx`: `slots.register({ name: 'shell.palette.source', plugin: 'projects' }, (p) => <ProjectsSource {...p} router={router} />)` inside `ctx.effect`.

`web/plugins/data-studio/src/palette.tsx`:

```tsx
import type { PaletteCommand } from '@loams/slots';
import { useEffect, useState } from 'react';
import type { DataClient } from './client.js';

/** The open environment's tables, by name, for the palette (D1078). Loaded once per opening. */
export function TablesSource({
  publish,
  client,
  namespace,
  navigate,
}: {
  query: string;
  publish: (commands: readonly PaletteCommand[]) => void;
  client: Pick<DataClient, 'listCollections'>;
  namespace: string | undefined;
  navigate: (to: string) => void;
}) {
  const [names, setNames] = useState<string[]>([]);
  useEffect(() => {
    if (!namespace) return;
    let live = true;
    client
      .listCollections(namespace)
      .then((cs) => live && setNames(cs.map((c) => c.name)))
      .catch(() => live && setNames([]));
    return () => {
      live = false;
    };
  }, [client, namespace]);
  useEffect(() => {
    if (!namespace) return publish([]);
    publish(
      names.map((name) => ({
        id: `table:${namespace}.${name}`,
        title: name,
        group: 'Tables',
        detail: namespace,
        keywords: [`${namespace}.${name}`],
        run: () => navigate(`/data/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}`),
      })),
    );
  }, [names, namespace, navigate, publish]);
  return null;
}
```

In `index.tsx`, register it with the plugin's `createDataClient(transport, platform.fetch, platform.baseUrl)` client and the session's environment:

```tsx
    const tablesClient = createDataClient(transport, platform.fetch, platform.baseUrl);
    ctx.effect(() =>
      slots.register({ name: 'shell.palette.source', plugin: 'data-studio' }, (p) => (
        <TablesSource {...p} client={tablesClient} namespace={session.environment()?.namespace} navigate={(to) => router.navigate(to)} />
      )),
    );
```

The data-studio plugin injects `desktop`, `router`, `slots`, `transport`, `platform` and `flags` today. Add `'session'` to `inject` in both `src/index.tsx:75` and `package.json`, read it with `const session = service(ctx, 'session');`, and add `shell.palette.source` to its `slots`.

`web/plugins/studio/src/palette/BranchesSource.tsx` (branches as navigation only: the palette never switches a checkout):

```tsx
import { AgentdScope, useAgentdQuery } from '@loams/console-host/query';
import { GitService } from '@loams/proto/agentd';
import type { PaletteCommand } from '@loams/slots';
import { useEffect } from 'react';
import type { StudioAgentd } from '../agentd.js';
import { useProjects, useReach } from '../projects/queries.js';

function ProjectBranches({
  project,
  publish,
  navigate,
}: {
  project: { id: string; name: string; path: string };
  publish: (id: string, commands: PaletteCommand[]) => void;
  navigate: (to: string) => void;
}) {
  const branches = useAgentdQuery(GitService.method.listBranches, { repoPath: project.path });
  useEffect(() => {
    publish(
      project.id,
      (branches.data?.branches ?? []).map((name) => ({
        id: `branch:${project.id}:${name}`,
        title: name,
        group: 'Branches',
        detail: project.name,
        keywords: [project.name],
        run: () => navigate(`/studio/p/${encodeURIComponent(project.id)}`),
      })),
    );
  }, [branches.data, project, publish, navigate]);
  return null;
}
```

`ListBranchesResponse.branches` is a `repeated string` (`proto/loams/agentd/v1/git.proto:188`). Then:

```tsx
function Branches({ agentd, publish, navigate }: { agentd: StudioAgentd; publish: (c: readonly PaletteCommand[]) => void; navigate: (to: string) => void }) {
  const reach = useReach(agentd);
  const projects = useProjects(agentd);
  const byProject = useRef(new Map<string, PaletteCommand[]>());
  const onProject = useCallback(
    (id: string, cs: PaletteCommand[]) => {
      byProject.current.set(id, cs);
      publish([...byProject.current.values()].flat());
    },
    [publish],
  );
  if (reach.state !== 'ready') return null;
  return (
    <>
      {(projects.data ?? []).map((p) => (
        <ProjectBranches key={p.id} project={p} publish={onProject} navigate={navigate} />
      ))}
    </>
  );
}

export function BranchesSource(props: { query: string; publish: (c: readonly PaletteCommand[]) => void; agentd: StudioAgentd; navigate: (to: string) => void }) {
  return (
    <AgentdScope agentd={props.agentd}>
      <Branches agentd={props.agentd} publish={props.publish} navigate={props.navigate} />
    </AgentdScope>
  );
}
```

Import `useCallback` and `useRef` from `react`. Register it in Studio's `index.tsx` as `shell.palette.source` with `meta: { mode: 'studio' }`.

`@loams/studio-ui` `palette/logic.ts`: add `preview?: string` to `PaletteThread`, and append `thread.preview ?? ''` as the last `keywords` entry in `buildThreadActionItems`. Its comment is "Last: a thread's own words rank under its title, project and branch (W13)". `plugins/studio/src/sidebar/paletteSource.ts`: `placedThreads` folds `Chat`s into `SidebarThread`s, which carry no preview. Make it also return `previews: new Map(frame.chats.map((c) => [c.id, (c.lastMessagePreview ?? '').slice(0, 500)]))` from the `watchThreads` frame it reads. Then `threads()` sets `preview: previews.get(t.id)` on each row.

- [ ] **Step 5: Run the tests**

```bash
pnpm --filter @loams/plugin-shell test
pnpm --filter @loams/plugin-projects test
pnpm --filter @loams/plugin-data-studio test
pnpm --filter @loams/plugin-studio test
pnpm --filter @loams/studio-ui test
pnpm -C web typecheck
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add web/packages/slots web/plugins/shell web/plugins/projects web/plugins/data-studio web/plugins/studio web/packages/studio-ui
git commit -s -m "feat(shell): palette sources for projects, branches, tables and thread text (CF1 task 11)"
```

---

### Task 12: One keymap in the shell (D1081)

**Files:**
- Create: `web/plugins/shell/src/keys.ts`, `web/plugins/shell/src/keyboard.tsx`
- Modify: `web/plugins/shell/src/layout.tsx` (Task 7's Mod+B listener with `keepsKeys`, Task 8's temporary listener and the palette's own Mod+K listener are removed; `useShellKeys` mounts), `src/palette.tsx` (`isPaletteShortcut` kept as an export, delegating to `matchShellKey`), `src/mode-switcher.tsx` (`useModeShortcut` stays: ⌘⇧M is D900's)
- Modify: `web/packages/studio-ui/src/keybindings/shared.ts:206-225` (the chords the shell owns are removed from `DEFAULT_KEYBINDINGS`), `src/keybindings/settings.ts:27-31` (`SHELL_KEYBINDINGS` lists D1081)
- Modify: `web/plugins/studio/src/palette/commands.ts:70-80` (`listenShellCommands` runs `thread.jump.N` and `thread.next` too)
- Modify: `web/packages/platform-electron/src/index.ts:205-234` (`wireShellCommands` forwards the new menu commands, Task 14)
- Create: `web/plugins/shell/test/keymap.test.ts` (Review Focus 1)
- Modify: `web/plugins/shell/package.json` (devDependency `@loams/studio-ui` for the parity test)

**Interfaces:**
- Consumes: `PanelService` (Task 8), `runShellCommand` (Task 7), `settingsHome` and `pageEntries` (Task 10), `SHELL_COMMAND_EVENT`.
- Produces: `ShellKeyCommand`, `ShellKeyContext`, `ShellKeyBinding`, `SHELL_KEYMAP`, `matchShellKey`, `keyLabel` (Shared contracts); `useShellKeys(o: ShellKeysOptions): void`; and `ShellKeysOptions = { router: HashRouter; inspector: PanelService; drawer: PanelService; toggleSidebar(): void; togglePalette(): void; openSettings(): void; closeOverlay(): boolean }`.

- [ ] **Step 1: Write the failing test**

`web/plugins/shell/test/keymap.test.ts`:

```ts
// D1081's table, and Review Focus 1: shortcuts typed into editable targets. Studio's rich-text
// composer keeps ⌘B/⌘I (bold, italic); a terminal keeps Ctrl+K/J/B/N/I on Linux and Windows
// (kill-line, newline, tmux's prefix, next history, Tab); Esc in an input is the input's.

import { SHELL_KEYBINDINGS } from '@loams/studio-ui';
import { describe, expect, it } from 'vitest';
import { keyLabel, matchShellKey, SHELL_KEYMAP, type ShellKeyContext } from '../src/keys.js';

const mac: ShellKeyContext = { mac: true, terminal: false, richText: false, editable: false };
const linux: ShellKeyContext = { ...mac, mac: false };
const key = (k: string, mods: Partial<KeyboardEvent> = {}) =>
  ({ key: k, code: '', metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...mods }) as KeyboardEvent;

describe('the keymap (D1081)', () => {
  it.each([
    [key('k', { metaKey: true }), 'palette.toggle'],
    [key('P', { metaKey: true, shiftKey: true }), 'palette.toggle'],
    [key('n', { metaKey: true }), 'thread.new'],
    [key('b', { metaKey: true }), 'sidebar.toggle'],
    [key('i', { metaKey: true }), 'inspector.toggle'],
    [key('j', { metaKey: true }), 'drawer.toggle'],
    [key('3', { metaKey: true }), 'thread.jump.3'],
    [key('Tab', { ctrlKey: true }), 'thread.next'],
    [key('[', { metaKey: true }), 'navigation.back'],
    [key(']', { metaKey: true }), 'navigation.forward'],
    [key('T', { metaKey: true, shiftKey: true }), 'thread.reopen'],
    [key(',', { metaKey: true }), 'settings.open'],
    [key('Escape'), 'overlay.close'],
  ] as const)('on macOS %o is %s', (e, command) => {
    expect(matchShellKey(e, mac)).toBe(command);
  });

  it('is Ctrl on Windows and Linux, and ⌘ does nothing there', () => {
    expect(matchShellKey(key('k', { ctrlKey: true }), linux)).toBe('palette.toggle');
    expect(matchShellKey(key('k', { metaKey: true }), linux)).toBeUndefined();
    expect(matchShellKey(key('k', { ctrlKey: true }), mac)).toBeUndefined();
    expect(keyLabel('mod+shift+p', true)).toEqual(['⌘', '⇧', 'P']);
    expect(keyLabel('mod+shift+p', false)).toEqual(['Ctrl', 'Shift', 'P']);
  });

  it('every binding is in one table, and Studio lists them for its terminal and settings', () => {
    expect(SHELL_KEYMAP.map((b) => b.key)).toEqual(
      expect.arrayContaining(['mod+k', 'mod+shift+p', 'mod+n', 'mod+b', 'mod+i', 'mod+j', 'ctrl+tab', 'mod+[', 'mod+]', 'mod+shift+t', 'mod+,', 'escape']),
    );
    const listed = new Set(SHELL_KEYBINDINGS.map((b) => b.key));
    for (const b of SHELL_KEYMAP) if (b.key !== 'escape') expect(listed.has(b.key), b.key).toBe(true);
  });
});

describe('editable targets keep their keys (Review Focus 1)', () => {
  it("the rich-text composer keeps ⌘B and ⌘I; the palette still opens", () => {
    const composer = { ...mac, richText: true, editable: true };
    expect(matchShellKey(key('b', { metaKey: true }), composer)).toBeUndefined();
    expect(matchShellKey(key('i', { metaKey: true }), composer)).toBeUndefined();
    expect(matchShellKey(key('k', { metaKey: true }), composer)).toBe('palette.toggle');
  });

  it("a terminal on Linux keeps Ctrl+K, J, B, N and I; Ctrl+Shift+P still opens the palette", () => {
    const term = { ...linux, terminal: true, editable: true };
    for (const k of ['k', 'j', 'b', 'n', 'i']) expect(matchShellKey(key(k, { ctrlKey: true }), term), k).toBeUndefined();
    expect(matchShellKey(key('P', { ctrlKey: true, shiftKey: true }), term)).toBe('palette.toggle');
  });

  it('a macOS terminal still gets ⌘ shortcuts (⌘ never reaches the shell program)', () => {
    expect(matchShellKey(key('j', { metaKey: true }), { ...mac, terminal: true, editable: true })).toBe('drawer.toggle');
  });

  it('Escape in an input is the input’s, and ⌘[ in a text field is not Back', () => {
    const field = { ...mac, editable: true };
    expect(matchShellKey(key('Escape'), field)).toBeUndefined();
    expect(matchShellKey(key('[', { metaKey: true }), field)).toBeUndefined();
  });

  it('a key with an extra modifier is not the binding', () => {
    expect(matchShellKey(key('k', { metaKey: true, altKey: true }), mac)).toBeUndefined();
    expect(matchShellKey(key('n', { metaKey: true, shiftKey: true }), mac)).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `pnpm --filter @loams/plugin-shell exec vitest run test/keymap.test.ts`
Expected: FAIL, with `Cannot find module '../src/keys.js'`. (Add `"@loams/studio-ui": "workspace:*"` to the shell's devDependencies first, and run `pnpm install`.)

- [ ] **Step 3: The table**

`web/plugins/shell/src/keys.ts`:

```ts
// The console's keyboard (design 59, D1081): one table, here. ⌘ is Ctrl on Windows and Linux;
// ⌃Tab is Control everywhere. Part 2 and later parts add rows to SHELL_KEYMAP; a user-editable
// keymap is Q930. `when` keeps a key with the focused editor or terminal (Review Focus 1).

export type ShellKeyCommand =
  | 'palette.toggle'
  | 'thread.new'
  | 'sidebar.toggle'
  | 'inspector.toggle'
  | 'drawer.toggle'
  | `thread.jump.${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9}`
  | 'thread.next'
  | 'navigation.back'
  | 'navigation.forward'
  | 'thread.reopen'
  | 'settings.open'
  | 'overlay.close';

export interface ShellKeyContext {
  mac: boolean;
  /** Focus is in a terminal (`[data-terminal]`). */
  terminal: boolean;
  /** Focus is in a rich-text editor (`[data-composer-rich-text="true"]`, contenteditable). */
  richText: boolean;
  /** Focus is in any text input, textarea, select or contenteditable. */
  editable: boolean;
}

export interface ShellKeyBinding {
  key: string;
  command: ShellKeyCommand;
  label: string;
  /** False keeps the key with the focused element. */
  when?: (c: ShellKeyContext) => boolean;
}

/** On Linux and Windows a terminal owns Ctrl+letter; on macOS ⌘ never reaches it. */
const notTerminal = (c: ShellKeyContext) => c.mac || !c.terminal;
const notRichText = (c: ShellKeyContext) => !c.richText;
const notEditable = (c: ShellKeyContext) => !c.editable;

export const SHELL_KEYMAP: readonly ShellKeyBinding[] = [
  { key: 'mod+k', command: 'palette.toggle', label: 'Command palette', when: notTerminal },
  { key: 'mod+shift+p', command: 'palette.toggle', label: 'Command palette' },
  { key: 'mod+n', command: 'thread.new', label: 'New thread', when: notTerminal },
  { key: 'mod+b', command: 'sidebar.toggle', label: 'Toggle sidebar', when: (c) => notTerminal(c) && notRichText(c) },
  { key: 'mod+i', command: 'inspector.toggle', label: 'Toggle inspector', when: (c) => notTerminal(c) && notRichText(c) },
  { key: 'mod+j', command: 'drawer.toggle', label: 'Toggle drawer', when: notTerminal },
  ...([1, 2, 3, 4, 5, 6, 7, 8, 9] as const).map((n) => ({
    key: `mod+${n}`,
    command: `thread.jump.${n}` as const,
    label: `Thread ${n}`,
    when: notTerminal,
  })),
  { key: 'ctrl+tab', command: 'thread.next', label: 'Next thread' },
  { key: 'mod+[', command: 'navigation.back', label: 'Back', when: (c) => notTerminal(c) && notEditable(c) },
  { key: 'mod+]', command: 'navigation.forward', label: 'Forward', when: (c) => notTerminal(c) && notEditable(c) },
  { key: 'mod+shift+t', command: 'thread.reopen', label: 'Reopen closed thread', when: notTerminal },
  { key: 'mod+,', command: 'settings.open', label: 'Settings' },
  { key: 'escape', command: 'overlay.close', label: 'Close the topmost overlay', when: notEditable },
];

interface Chord {
  key: string;
  mod: boolean;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
}

function parse(spec: string): Chord {
  const parts = spec.split('+');
  const key = parts.at(-1) ?? '';
  return { key, mod: parts.includes('mod'), ctrl: parts.includes('ctrl'), shift: parts.includes('shift'), alt: parts.includes('alt') };
}

const CHORDS = SHELL_KEYMAP.map((b) => [b, parse(b.key)] as const);

export function matchShellKey(
  e: Pick<KeyboardEvent, 'key' | 'code' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>,
  ctx: ShellKeyContext,
): ShellKeyCommand | undefined {
  const pressed = e.key.toLowerCase();
  for (const [binding, c] of CHORDS) {
    const wantMeta = c.mod && ctx.mac;
    const wantCtrl = c.ctrl || (c.mod && !ctx.mac);
    if (e.metaKey !== wantMeta || e.ctrlKey !== wantCtrl || e.shiftKey !== c.shift || e.altKey !== c.alt) continue;
    if (pressed !== c.key) continue;
    if (binding.when && !binding.when(ctx)) return undefined;
    return binding.command;
  }
  return undefined;
}

export function keyLabel(spec: string, mac: boolean): string[] {
  return spec.split('+').map((p) => {
    if (p === 'mod') return mac ? '⌘' : 'Ctrl';
    if (p === 'ctrl') return mac ? '⌃' : 'Ctrl';
    if (p === 'shift') return mac ? '⇧' : 'Shift';
    if (p === 'alt') return mac ? '⌥' : 'Alt';
    if (p === 'escape') return 'Esc';
    if (p === 'tab') return 'Tab';
    return p.length === 1 ? p.toUpperCase() : p;
  });
}
```

`matchShellKey` returns `undefined` as soon as the matching chord's `when` fails, so a later row with the same chord cannot catch it. The table has no duplicate chords, so this is the intended behaviour.

- [ ] **Step 4: One listener**

`web/plugins/shell/src/keyboard.tsx`:

```tsx
// The shell's one keydown listener (D1081): it resolves the event against SHELL_KEYMAP and
// runs the command. Studio's commands travel as SHELL_COMMAND_EVENT (thread.new → chat.new,
// thread.jump.N, thread.next); the shell keeps ⌘⇧T's history itself (W12).

import type { PanelService } from '@loams/console-host';
import { MODE_HOME } from '@loams/console-host';
import { useEffect, useRef } from 'react';
import { runShellCommand } from './frame.js';
import { matchShellKey, type ShellKeyContext } from './keys.js';
import type { HashRouter } from './router.js';

export interface ShellKeysOptions {
  router: HashRouter;
  inspector: PanelService;
  drawer: PanelService;
  toggleSidebar(): void;
  togglePalette(): void;
  openSettings(): void;
  /** Closes the shell's own topmost overlay (the narrow sidebar sheet); false if none was open. */
  closeOverlay(): boolean;
}

export function contextOf(target: EventTarget | null): Omit<ShellKeyContext, 'mac'> {
  const el = target instanceof Element ? target : null;
  const editable = Boolean(el?.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]'));
  return {
    terminal: Boolean(el?.closest('[data-terminal]')),
    richText: Boolean(el?.closest('[data-composer-rich-text="true"], [contenteditable=""], [contenteditable="true"]')),
    editable,
  };
}

const THREAD = /^\/studio\/t\/[^/]+/;

export function useShellKeys(o: ShellKeysOptions): void {
  const latest = useRef(o);
  latest.current = o;
  const left = useRef<string[]>([]);

  // ⌘⇧T: remember thread routes as they are left (W12).
  useEffect(() => {
    let previous = o.router.current().path;
    return o.router.subscribe(() => {
      const now = o.router.current().path;
      if (THREAD.test(previous) && now !== previous) left.current = [...left.current.filter((p) => p !== previous), previous].slice(-20);
      previous = now;
    });
  }, [o.router]);

  useEffect(() => {
    const mac = /mac|iphone|ipad/i.test(globalThis.navigator?.platform ?? '');
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const command = matchShellKey(e, { mac, ...contextOf(e.target) });
      if (!command) return;
      const x = latest.current;
      const done = () => e.preventDefault();
      switch (command) {
        case 'palette.toggle':
          done();
          return x.togglePalette();
        case 'thread.new':
          done();
          return runShellCommand('chat.new', () => x.router.navigate(MODE_HOME.studio));
        case 'sidebar.toggle':
          done();
          return x.toggleSidebar();
        case 'inspector.toggle':
          done();
          return x.inspector.toggle();
        case 'drawer.toggle':
          done();
          return x.drawer.toggle();
        case 'thread.next':
          done();
          return runShellCommand('thread.next', () => {});
        case 'navigation.back':
          done();
          return globalThis.history.back();
        case 'navigation.forward':
          done();
          return globalThis.history.forward();
        case 'thread.reopen': {
          done();
          const last = left.current.pop();
          if (last) x.router.navigate(last);
          return;
        }
        case 'settings.open':
          done();
          return x.openSettings();
        case 'overlay.close':
          if (x.closeOverlay()) done();
          return;
        default:
          // thread.jump.N
          done();
          return runShellCommand(command, () => {});
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
}
```

Base UI dialogs, menus and the palette handle Escape themselves and stop it, so `e.defaultPrevented` keeps the shell out of their way. The shell's `overlay.close` only closes the narrow sidebar sheet or the narrow inspector sheet.

In `Layout`:
1. Delete Task 7's Mod+B `useEffect` and its `keepsKeys` helper (T7-5), Task 8's temporary listener and its `keepsPanelKeys` helper (T8-2), and `ShellPalette`'s own Mod+K `useEffect`.
2. Call:

```tsx
  useShellKeys({
    router,
    inspector,
    drawer,
    toggleSidebar,
    togglePalette: () => setPaletteOpen((v) => !v),
    openSettings: () => {
      const home = settingsHome(pageEntries(slotsRegistry.entries('shell.nav.section'), slotsRegistry.entries('console.nav'), currentProject(router.current().path)));
      if (home) router.navigate(home.href);
    },
    closeOverlay: () => {
      if (narrow && sheetOpen) {
        setSheetOpen(false);
        return true;
      }
      if (narrow && inspector.state().open) {
        inspector.close();
        return true;
      }
      return false;
    },
  });
```

`slotsRegistry` is `useSlotRegistry()` from `@loams/slots`. `isPaletteShortcut(e)` stays exported, re-implemented as `matchShellKey(e, { mac: /mac/i.test(navigator.platform), terminal: false, richText: false, editable: false }) === 'palette.toggle'`.

- [ ] **Step 5: Studio gives up the shell's chords**

In `@loams/studio-ui` `keybindings/shared.ts`, remove these rows from `DEFAULT_KEYBINDINGS`: `mod+b` `sidebar.toggle`, `mod+[` and `mod+]`, `mod+n` `chat.new`, `mod+,` `settings.open`, and the nine `mod+N` thread jumps. Studio still runs those commands when the shell sends them. Keep `mod+shift+o` (`chat.new`), `ctrl+\`` (`terminal.toggle`, now the drawer's Terminal), `mod+alt+b`, `mod+d`, `mod+shift+e`, `mod+shift+j` and `mod+shift+[`/`]`. Update the doc comment above the table to say the shell owns D1081's chords. `settings.ts`'s `SHELL_KEYBINDINGS` becomes D1081's list:

```ts
export const SHELL_KEYBINDINGS: ReadonlyArray<{ readonly key: string; readonly label: string }> = [
  { key: "mod+k", label: "Shell: Command Palette" },
  { key: "mod+shift+p", label: "Shell: Command Palette" },
  { key: "mod+n", label: "Shell: New Thread" },
  { key: "mod+b", label: "Shell: Toggle Sidebar" },
  { key: "mod+i", label: "Shell: Toggle Inspector" },
  { key: "mod+j", label: "Shell: Toggle Drawer" },
  ...[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => ({ key: `mod+${n}`, label: `Shell: Thread ${n}` })),
  { key: "ctrl+tab", label: "Shell: Next Thread" },
  { key: "mod+[", label: "Shell: Back" },
  { key: "mod+]", label: "Shell: Forward" },
  { key: "mod+shift+t", label: "Shell: Reopen Closed Thread" },
  { key: "mod+,", label: "Shell: Settings" },
  { key: "mod+shift+m", label: "Shell: Switch Mode" },
];
```

Studio's terminal passes these chords through to the shell on macOS only. That matches the `notTerminal` rule, since a Linux terminal keeps Ctrl+letter. In `TerminalDrawer.tsx`, filter `SHELL_SHORTCUTS` to `metaKey` chords when `navigator.platform` is not a Mac. In `plugins/studio/src/palette/commands.ts`, `listenShellCommands` runs any Studio command it is sent:

```ts
  const onCommand = (event: Event) => {
    const detail = (event as CustomEvent<unknown>).detail;
    if (typeof detail === 'string' && isKeybindingCommand(detail) && commands.run(detail)) event.preventDefault();
  };
```

`isKeybindingCommand` comes from `@loams/studio-ui`. `chat.new`, `thread.next` and `thread.jump.N` are Studio command names, so the shell's events map one to one.

- [ ] **Step 6: Run the tests**

```bash
pnpm --filter @loams/plugin-shell test
pnpm --filter @loams/studio-ui test
pnpm --filter @loams/plugin-studio test
pnpm --filter @loams/platform-electron test
pnpm -C web typecheck
```

Expected: PASS. Studio's keybinding tests that asserted the removed rows now assert their absence, and that the shell's events still run those commands.

- [ ] **Step 7: Commit**

```bash
git add web/plugins/shell web/packages/studio-ui web/plugins/studio web/packages/platform-electron
git commit -s -m "feat(shell): one keymap for D1081, editable targets keep their keys (CF1 task 12)"
```

---

## CF1d — Desktop chrome (D1083–D1085)

### Task 13: One window style per OS, synced to the page theme

**Files:**
- Create: `apps/desktop-electron/src/main/shell/window-chrome.ts` (pure, adapted from dsh-desktop `src/main/index.ts` `createWindow`/`windowsTitleBarOverlay`/`applyWindowChromeTheme`)
- Create: `apps/desktop-electron/src/main/shell/macos-window-backdrop.ts` (adapted from dsh-desktop `src/main/macos-window-backdrop.ts`)
- Create: `apps/desktop-electron/src/main/shell/window-chrome.electron.ts` (events and IPC)
- Create: `apps/desktop-electron/src/preload/window-chrome.ts` (adapted from dsh-desktop `src/preload/macos-window-chrome.ts`)
- Modify: `apps/desktop-electron/src/main/shell/window-state.ts:25` (`MIN_HEIGHT = 640`), `test/window-state.test.ts` (its 600 expectation)
- Modify: `apps/desktop-electron/src/main/index.ts:134-200` (`createWindow`), and the `registerThemeIpc` call from Task 2 (`onTheme`)
- Modify: `apps/desktop-electron/src/shared/contracts.ts` (`CH.shellFullscreen`), `src/preload/index.ts` (import the chrome marker)
- Modify: `web/apps/console/src/cordis/console.css` (the platform rules)
- Create: `apps/desktop-electron/test/window-chrome.test.ts` (Review Focus 3)

**Interfaces:**
- Consumes:
  - Task 2's `registerThemeIpc({ settingsFile, onTheme })` and `readThemeChoice`.
  - Task 7's `--lc-traffic`, `--lc-traffic-top` and `--lc-wco-end`, and the `.lc-topbar` drag region.
- Produces:
  - `TOPBAR_HEIGHT = 44` and `TRAFFIC_LIGHT = { x: 16, y: 15 }`.
  - `CHROME: { dark: { window: '#111111'; canvas: '#0a0a0a'; symbol: '#f5f5f5' }; light: { window: '#f4f2ee'; canvas: '#ffffff'; symbol: '#1d1b18' } }`.
  - `windowChromeOptions(platform: NodeJS.Platform, dark: boolean): BrowserWindowConstructorOptions` and `titleBarOverlay(dark: boolean): TitleBarOverlayOptions`.
  - `macBackdrop(s: { minimized: boolean; visible: boolean }, dark: boolean): { vibrancy: 'sidebar' | null; background: string }`.
  - `ChromeWindow`, `applyWindowChrome(win: ChromeWindow, platform: NodeJS.Platform, dark: boolean): void` and `ChromeSync`.
  - `installWindowChrome(win: BrowserWindow, sync: ChromeSync): void`.

- [ ] **Step 1: Write the failing test**

`apps/desktop-electron/test/window-chrome.test.ts`:

```ts
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { MIN_HEIGHT, MIN_WIDTH } from "../src/main/shell/window-state";
import {
	CHROME,
	type ChromeWindow,
	ChromeSync,
	TRAFFIC_LIGHT,
	macBackdrop,
	titleBarOverlay,
	windowChromeOptions,
} from "../src/main/shell/window-chrome";

function fakeWindow(o: { minimized?: boolean; visible?: boolean; destroyed?: boolean } = {}) {
	const calls: [string, unknown][] = [];
	const win: ChromeWindow = {
		isDestroyed: () => o.destroyed ?? false,
		isMinimized: () => o.minimized ?? false,
		isVisible: () => o.visible ?? true,
		setVibrancy: (v) => void calls.push(["vibrancy", v]),
		setBackgroundColor: (c) => void calls.push(["background", c]),
		setTitleBarOverlay: (x) => void calls.push(["overlay", x]),
	};
	return { win, calls };
}

describe("window chrome per OS (D1083)", () => {
	it("macOS: hidden inset, traffic lights at x 16 centred in the 44px bar, sidebar vibrancy", () => {
		const o = windowChromeOptions("darwin", true);
		expect(o).toMatchObject({
			titleBarStyle: "hiddenInset",
			trafficLightPosition: { x: 16, y: 15 },
			vibrancy: "sidebar",
			visualEffectState: "active",
			backgroundColor: "#00000000",
		});
		expect(TRAFFIC_LIGHT.y + 7).toBe(22);
	});

	it.each(["win32", "linux"] as const)("%s: hidden with a 44px title bar overlay in the theme's colours", (platform) => {
		expect(windowChromeOptions(platform, true)).toMatchObject({
			titleBarStyle: "hidden",
			titleBarOverlay: { color: "#111111", symbolColor: "#f5f5f5", height: 44 },
			backgroundColor: "#111111",
			autoHideMenuBar: true,
		});
		expect(titleBarOverlay(false)).toEqual({ color: "#f4f2ee", symbolColor: "#1d1b18", height: 44 });
	});

	it("keeps a minimum of 900×640", () => {
		expect([MIN_WIDTH, MIN_HEIGHT]).toEqual([900, 640]);
	});

	it("uses the same colours as the tokens", () => {
		const css = readFileSync(resolve(__dirname, "../../../web/packages/ui/src/tokens.css"), "utf8");
		const light = css.slice(css.indexOf("\n:root {"), css.indexOf("\n.dark {"));
		const dark = css.slice(css.indexOf("\n.dark {"));
		const token = (block: string, name: string) => new RegExp(`${name}: (#[0-9a-f]{6});`).exec(block)?.[1];
		expect(CHROME.light).toEqual({ window: token(light, "--op-window"), canvas: token(light, "--op-bg"), symbol: token(light, "--op-ink") });
		expect(CHROME.dark).toEqual({ window: token(dark, "--op-window"), canvas: token(dark, "--op-bg"), symbol: token(dark, "--op-ink") });
	});
});

describe("chrome follows every theme change (Review Focus 3)", () => {
	it("Windows and Linux: the overlay and background update on each switch", () => {
		const { win, calls } = fakeWindow();
		let choice: "system" | "dark" | "light" = "dark";
		const sync = new ChromeSync({ platform: "linux", window: () => win, choice: () => choice, osDark: () => false });
		sync.onTheme(true);
		sync.onTheme(false);
		expect(calls).toEqual([
			["background", "#111111"],
			["overlay", { color: "#111111", symbolColor: "#f5f5f5", height: 44 }],
			["background", "#f4f2ee"],
			["overlay", { color: "#f4f2ee", symbolColor: "#1d1b18", height: 44 }],
		]);
		calls.length = 0;
		sync.onOsThemeChanged(); // an explicit choice ignores the OS
		expect(calls).toEqual([]);
		choice = "system";
		sync.onOsThemeChanged(); // System follows it (the OS says light here)
		expect(calls.at(-1)).toEqual(["overlay", { color: "#f4f2ee", symbolColor: "#1d1b18", height: 44 }]);
	});

	it("macOS: vibrancy while visible, a solid backdrop while hidden or minimised", () => {
		expect(macBackdrop({ minimized: false, visible: true }, true)).toEqual({ vibrancy: "sidebar", background: "#00000000" });
		expect(macBackdrop({ minimized: true, visible: true }, true)).toEqual({ vibrancy: null, background: "#111111" });
		expect(macBackdrop({ minimized: false, visible: false }, false)).toEqual({ vibrancy: null, background: "#f4f2ee" });
		const { win, calls } = fakeWindow({ minimized: true });
		new ChromeSync({ platform: "darwin", window: () => win, choice: () => "dark", osDark: () => true }).onTheme(true);
		expect(calls).toEqual([["vibrancy", null], ["background", "#111111"]]);
	});

	it("leaves a destroyed window alone", () => {
		const { win, calls } = fakeWindow({ destroyed: true });
		new ChromeSync({ platform: "win32", window: () => win, choice: () => "dark", osDark: () => true }).onTheme(false);
		expect(calls).toEqual([]);
	});
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `pnpm --filter @loams/desktop exec vitest run test/window-chrome.test.ts`
Expected: FAIL, with `Cannot find module '../src/main/shell/window-chrome'`.

- [ ] **Step 3: The pure chrome module**

`apps/desktop-electron/src/main/shell/window-chrome.ts`:

```ts
// Adapted from dataelement/dsh-desktop (MIT), src/main/index.ts.
// One window style per OS (design 59, D1083): macOS gets a hidden inset title bar with the
// traffic lights centred in the 44px top bar and sidebar vibrancy; Windows and Linux get a
// hidden title bar with a 44px overlay whose colours follow the page theme. Upstream's
// createWindow options, windowsTitleBarOverlay and applyWindowChromeTheme, made pure; the
// values are Loams' (the tokens, a test compares them). Linux needs an opaque overlay colour
// (as T3 Code notes, #00000000 does not work there).

import type { BrowserWindow, BrowserWindowConstructorOptions, TitleBarOverlayOptions } from "electron";
import type { ThemeChoice } from "../../shared/contracts";

export const TOPBAR_HEIGHT = 44;
const MAC_BUTTON_RADIUS = 7;
export const TRAFFIC_LIGHT = { x: 16, y: TOPBAR_HEIGHT / 2 - MAC_BUTTON_RADIUS } as const;

/** --op-window, --op-bg and --op-ink from web/packages/ui/src/tokens.css (test/window-chrome.test.ts). */
export const CHROME = {
	dark: { window: "#111111", canvas: "#0a0a0a", symbol: "#f5f5f5" },
	light: { window: "#f4f2ee", canvas: "#ffffff", symbol: "#1d1b18" },
} as const;

export function titleBarOverlay(dark: boolean): TitleBarOverlayOptions {
	const c = dark ? CHROME.dark : CHROME.light;
	return { color: c.window, symbolColor: c.symbol, height: TOPBAR_HEIGHT };
}

export function windowChromeOptions(platform: NodeJS.Platform, dark: boolean): BrowserWindowConstructorOptions {
	if (platform === "darwin") {
		return {
			titleBarStyle: "hiddenInset",
			trafficLightPosition: { ...TRAFFIC_LIGHT },
			vibrancy: "sidebar",
			visualEffectState: "active",
			backgroundColor: "#00000000",
		};
	}
	return {
		titleBarStyle: "hidden",
		titleBarOverlay: titleBarOverlay(dark),
		backgroundColor: (dark ? CHROME.dark : CHROME.light).window,
		autoHideMenuBar: true,
	};
}

/** dsh-desktop's backdrop swap: vibrancy while shown, a solid colour while hidden (no flash). */
export function macBackdrop(
	s: { minimized: boolean; visible: boolean },
	dark: boolean,
): { vibrancy: "sidebar" | null; background: string } {
	if (s.minimized || !s.visible) return { vibrancy: null, background: (dark ? CHROME.dark : CHROME.light).window };
	return { vibrancy: "sidebar", background: "#00000000" };
}

export type ChromeWindow = Pick<
	BrowserWindow,
	"isDestroyed" | "isMinimized" | "isVisible" | "setVibrancy" | "setBackgroundColor" | "setTitleBarOverlay"
>;

export function applyWindowChrome(win: ChromeWindow, platform: NodeJS.Platform, dark: boolean): void {
	if (win.isDestroyed()) return;
	if (platform === "darwin") {
		const b = macBackdrop({ minimized: win.isMinimized(), visible: win.isVisible() }, dark);
		win.setVibrancy(b.vibrancy);
		win.setBackgroundColor(b.background);
		return;
	}
	win.setBackgroundColor((dark ? CHROME.dark : CHROME.light).window);
	win.setTitleBarOverlay(titleBarOverlay(dark));
}

/** Keeps the native chrome on the page theme: each `theme:set`, and each OS flip under System. */
export class ChromeSync {
	#dark: boolean;

	constructor(
		private readonly o: {
			platform: NodeJS.Platform;
			window: () => ChromeWindow | undefined;
			choice: () => ThemeChoice;
			osDark: () => boolean;
		},
	) {
		this.#dark = o.choice() === "dark" || (o.choice() === "system" && o.osDark());
	}

	get dark(): boolean {
		return this.#dark;
	}

	onTheme(dark: boolean): void {
		this.#dark = dark;
		this.apply();
	}

	onOsThemeChanged(): void {
		if (this.o.choice() !== "system") return;
		this.onTheme(this.o.osDark());
	}

	/** Re-applies the current theme (the macOS backdrop on hide, show, minimise and restore). */
	apply(): void {
		const win = this.o.window();
		if (win) applyWindowChrome(win, this.o.platform, this.#dark);
	}
}
```

`apps/desktop-electron/src/main/shell/macos-window-backdrop.ts`:

```ts
// Adapted from dataelement/dsh-desktop (MIT), src/main/macos-window-backdrop.ts.
// Match the sidebar's native material and avoid transparent restore flashes (D1083).
import type { BrowserWindow } from "electron";
import { macBackdrop } from "./window-chrome";

export function applyMacosWindowBackdrop(
	window: Pick<BrowserWindow, "isDestroyed" | "isMinimized" | "isVisible" | "setVibrancy" | "setBackgroundColor">,
	dark: boolean,
): void {
	if (window.isDestroyed()) return;
	const b = macBackdrop({ minimized: window.isMinimized(), visible: window.isVisible() }, dark);
	window.setVibrancy(b.vibrancy);
	window.setBackgroundColor(b.background);
}
```

`applyWindowChrome`'s darwin branch calls `applyMacosWindowBackdrop(win, dark)` instead of repeating it.

- [ ] **Step 4: Wire it into the window**

`apps/desktop-electron/src/main/shell/window-chrome.electron.ts`:

```ts
import type { BrowserWindow } from "electron";
import { CH } from "../../shared/contracts";
import type { ChromeSync } from "./window-chrome";

/** The backdrop swap on macOS, and data-fullscreen for the page (D1083). */
export function installWindowChrome(win: BrowserWindow, sync: ChromeSync): void {
	if (process.platform === "darwin") {
		for (const ev of ["minimize", "hide", "restore", "show"] as const) win.on(ev as "show", () => sync.apply());
	}
	const fullscreen = (): void => {
		if (!win.isDestroyed()) win.webContents.send(CH.shellFullscreen, win.isFullScreen());
	};
	win.webContents.on("did-finish-load", fullscreen);
	win.on("enter-full-screen", fullscreen);
	win.on("leave-full-screen", fullscreen);
}
```

In `index.ts`, declare `let chrome: ChromeSync | undefined;` beside the other module-level handles (`engine`, `tray`). Then:

1. In `app.whenReady`, right after Task 2's `nativeTheme.themeSource = readThemeChoice(settingsFile());`:

```ts
			chrome = new ChromeSync({
				platform: process.platform,
				window: getMainWindow,
				choice: () => readThemeChoice(settingsFile()),
				osDark: () => nativeTheme.shouldUseDarkColors,
			});
			const sync = chrome;
			nativeTheme.on("updated", () => sync.onOsThemeChanged());
```

   Change Task 2's `registerThemeIpc` call to `onTheme: (dark) => sync.onTheme(dark)`.
2. In `createWindow`, spread `...windowChromeOptions(process.platform, chrome?.dark ?? nativeTheme.shouldUseDarkColors)` into the `BrowserWindow` options before `webPreferences`, and after `secureWindow(win)` call `if (chrome) installWindowChrome(win, chrome);`.
3. Add `CH.shellFullscreen = "shell:fullscreen"` to `contracts.ts`, and `onFullscreen(cb)` to the `shell` API type.

`apps/desktop-electron/src/preload/window-chrome.ts`:

```ts
// Adapted from dataelement/dsh-desktop (MIT), src/preload/macos-window-chrome.ts.
// Marks <html> with the OS and the fullscreen state, so the console's CSS reserves the traffic
// lights' or the title bar controls' space only when it is there (D1083).
import { ipcRenderer } from "electron";
import { CH } from "../shared/contracts";

export function mountWindowChrome(doc: Document): () => void {
	let fullscreen = false;
	const mark = (): void => {
		const root = doc.documentElement;
		if (!root) return;
		root.dataset.platform = process.platform;
		if (fullscreen) root.dataset.fullscreen = "true";
		else delete root.dataset.fullscreen;
	};
	mark();
	doc.addEventListener("DOMContentLoaded", mark, { once: true });
	const receive = (_e: unknown, value: unknown): void => {
		if (typeof value !== "boolean") return;
		fullscreen = value;
		mark();
	};
	ipcRenderer.on(CH.shellFullscreen, receive);
	return () => ipcRenderer.removeListener(CH.shellFullscreen, receive);
}
```

In `src/preload/index.ts`, before `exposeInMainWorld`: `const unmountChrome = mountWindowChrome(document); window.addEventListener("unload", unmountChrome, { once: true });`. Add `onFullscreen: (cb) => subscribe(CH.shellFullscreen, cb)` to `api.shell`. In `fake-desktop.ts`, add `onFullscreen: () => () => {}`.

- [ ] **Step 5: The platform CSS**

Append to `console.css`:

```css
/* Desktop chrome (D1083): the preload sets data-platform and data-fullscreen on <html>. */
html[data-platform="darwin"]:not([data-fullscreen="true"]) .lc-shell {
  --lc-traffic: 76px;
  --lc-traffic-top: var(--op-topbar);
}
html[data-platform="darwin"] .lc-side-head {
  -webkit-app-region: drag;
}
html[data-platform="darwin"] .lc-side-head :is(button, a, input, [role="combobox"]) {
  -webkit-app-region: no-drag;
}
/* Vibrancy behind the sidebar only (Q933: in light too); the top bar and work area stay solid. */
html[data-platform="darwin"],
html[data-platform="darwin"] body,
html[data-platform="darwin"] .lc-shell {
  background: transparent;
}
html[data-platform="darwin"] .lc-side {
  background: transparent;
}
html[data-platform="darwin"] .lc-body {
  background: var(--op-window);
}
/* Windows and Linux: keep clear of the title bar overlay's buttons. */
html[data-platform="win32"] .lc-shell,
html[data-platform="linux"] .lc-shell {
  --lc-wco-end: calc(100vw - env(titlebar-area-x, 0px) - env(titlebar-area-width, 100vw));
}
```

Change `window-state.ts:25` to `export const MIN_HEIGHT = 640;`, and update `test/window-state.test.ts` where it expects 600.

- [ ] **Step 6: Run the tests**

```bash
pnpm --filter @loams/desktop exec vitest run test/window-chrome.test.ts test/window-state.test.ts
pnpm --filter @loams/desktop test
pnpm --filter @loams/desktop typecheck
```

Expected: PASS.

- [ ] **Step 7: Look at it on the desktop**

```bash
pnpm --filter @loams/console build
pnpm --filter @loams/desktop dev
```

On Linux, check four things. The title bar controls sit inside the top bar at the right, with no gap under them. Switching Appearance → Light recolours them at once. Choosing System and flipping the desktop's dark mode recolours them. Dragging the top bar moves the window, and its buttons still click. Record the macOS and Windows checks as a manual step for the release checklist; CI runs Linux only.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop-electron web/apps/console/src/cordis
git commit -s -m "feat(desktop): window chrome per OS, synced to the page theme (CF1 task 13)"
```

---

### Task 14: Native menus and the context menu

**Files:**
- Modify: `apps/desktop-electron/src/main/shell/menu-model.ts` (whole template), `src/main/shell/menu.electron.ts` (unchanged mapping)
- Create: `apps/desktop-electron/src/main/shell/context-menu-template.ts` (adapted from dsh-desktop `src/main/context-menu-template.ts`), `src/main/shell/context-menu.electron.ts` (adapted from `src/main/context-menu.ts`)
- Modify: `apps/desktop-electron/src/main/index.ts:350-362` (menu actions), `src/shared/contracts.ts:615-619` (`ShellCommand`)
- Modify: `apps/desktop-electron/src/main/update/updater.electron.ts` (return `check`)
- Modify: `web/packages/platform-electron/src/index.ts:205-234` (`wireShellCommands`), `web/plugins/shell/src/keyboard.tsx` (shell commands from the menu)
- Modify: `apps/desktop-electron/test/menu-model.test.ts`; create `apps/desktop-electron/test/context-menu.test.ts`
- Modify: `web/packages/platform-electron/test/shell-commands.test.ts` (create if absent)

**Interfaces:**
- Consumes: Task 12's `SHELL_KEYMAP` commands, `useShellKeys`; the updater; the engine supervisor.
- Produces:
  - `ShellCommand` gains `'palette.toggle' | 'sidebar.toggle' | 'inspector.toggle' | 'drawer.toggle' | 'navigation.back' | 'navigation.forward' | 'settings.open' | 'thread.next' | 'thread.reopen'`.
  - `MENU_COMMANDS` gains the matching ids.
  - The main-only ids `check-updates`, `restart-server`, `show-logs`, `report-issue` and `ISSUES_URL`.
  - `buildContextMenuTemplate(state: ContextMenuState, platform: NodeJS.Platform, actions: ContextMenuActions): MenuItemConstructorOptions[]` and `installContextMenu(win: BrowserWindow): void`.

- [ ] **Step 1: Write the failing tests**

Replace `apps/desktop-electron/test/menu-model.test.ts`'s cases with these, and keep its `flat`/`item` helpers:

```ts
describe("menu model (D1084)", () => {
	it("has App on macOS, then File, Edit, View, Window and Help", () => {
		expect(menuModel({ platform: "darwin", dev: false }).map((m) => m.label)).toEqual([
			"Loams Desktop", "File", "Edit", "View", "Window", "Help",
		]);
		expect(menuModel({ platform: "linux", dev: false }).map((m) => m.label)).toEqual([
			"File", "Edit", "View", "Window", "Help",
		]);
	});

	it.each([
		["new-thread", "CmdOrCtrl+N", "chat.new"],
		["palette", "CmdOrCtrl+K", "palette.toggle"],
		["toggle-sidebar", "CmdOrCtrl+B", "sidebar.toggle"],
		["toggle-inspector", "CmdOrCtrl+I", "inspector.toggle"],
		["toggle-drawer", "CmdOrCtrl+J", "drawer.toggle"],
		["back", "CmdOrCtrl+[", "navigation.back"],
		["forward", "CmdOrCtrl+]", "navigation.forward"],
		["next-thread", "Ctrl+Tab", "thread.next"],
		["reopen-thread", "CmdOrCtrl+Shift+T", "thread.reopen"],
		["settings", "CmdOrCtrl+,", "settings.open"],
		["mode-toggle", "CmdOrCtrl+Shift+M", "mode.toggle"],
	])("%s shows %s and the renderer runs %s (the shell owns the chord)", (id, accelerator, command) => {
		for (const platform of ["linux", "darwin", "win32"]) {
			expect(item(platform, id)).toMatchObject({ accelerator, registerAccelerator: false });
		}
		expect(menuCommand(id)).toBe(command);
	});

	it("has the app's own items, handled in main", () => {
		for (const id of ["check-updates", "restart-server", "show-logs", "report-issue"]) {
			for (const platform of ["linux", "darwin", "win32"]) expect(item(platform, id), `${platform} ${id}`).toBeTruthy();
			expect(menuCommand(id)).toBeUndefined();
		}
		expect(ISSUES_URL).toBe("https://github.com/ostrium-labs/loams/issues/new/choose");
	});
});
```

`apps/desktop-electron/test/context-menu.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { buildContextMenuTemplate, type ContextMenuState } from "../src/main/shell/context-menu-template";

const base: ContextMenuState = {
	isEditable: false,
	selectionText: "",
	linkURL: "",
	hasImageContents: false,
	editFlags: { canUndo: false, canRedo: false, canCut: false, canCopy: false, canPaste: false, canSelectAll: true },
};
const actions = { openLink: vi.fn(), copyLink: vi.fn(), copyImage: vi.fn(), lookUp: vi.fn() };
const labels = (t: ReturnType<typeof buildContextMenuTemplate>) => t.filter((i) => i.type !== "separator").map((i) => i.label);

describe("the context menu (D1084)", () => {
	it("offers cut, copy and paste in a field", () => {
		const t = buildContextMenuTemplate({ ...base, isEditable: true, editFlags: { ...base.editFlags, canCut: true, canCopy: true, canPaste: true } }, "linux", actions);
		expect(labels(t)).toEqual(["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"]);
	});

	it("offers Look Up for a selection on macOS only", () => {
		const sel = { ...base, selectionText: "branching" };
		expect(labels(buildContextMenuTemplate(sel, "darwin", actions))).toEqual(["Look Up “branching”", "Copy", "Select All"]);
		expect(labels(buildContextMenuTemplate(sel, "linux", actions))).toEqual(["Copy", "Select All"]);
	});

	it("opens only external web links in the browser", () => {
		expect(labels(buildContextMenuTemplate({ ...base, linkURL: "https://loams.dev" }, "linux", actions))[0]).toBe("Open Link in Browser");
		expect(labels(buildContextMenuTemplate({ ...base, linkURL: "http://127.0.0.1:8080" }, "linux", actions))[0]).toBe("Copy Link Address");
	});
});
```

`web/packages/platform-electron/test/shell-commands.test.ts`:

```ts
import { ModeController, SHELL_COMMAND_EVENT } from '@loams/console-host';
import { describe, expect, it, vi } from 'vitest';
import { wireShellCommands } from '../src/index.js';

describe('menu commands reach the shell (D1084)', () => {
  it('forwards each shell command as SHELL_COMMAND_EVENT', () => {
    let send: (c: string) => void = () => {};
    const api = { shell: { onCommand: (cb: (c: string) => void) => ((send = cb), () => {}) } } as never;
    const mode = new ModeController({ current: 'console', available: ['console'], modes: new Map(), navigate: vi.fn(), enter: vi.fn() });
    const target = new EventTarget();
    const seen: unknown[] = [];
    target.addEventListener(SHELL_COMMAND_EVENT, (e) => seen.push((e as CustomEvent).detail));
    wireShellCommands(api, mode, target);
    for (const c of ['palette.toggle', 'sidebar.toggle', 'inspector.toggle', 'drawer.toggle', 'navigation.back', 'settings.open', 'thread.next', 'thread.reopen']) send(c);
    expect(seen).toEqual(['palette.toggle', 'sidebar.toggle', 'inspector.toggle', 'drawer.toggle', 'navigation.back', 'settings.open', 'thread.next', 'thread.reopen']);
  });
});
```

- [ ] **Step 2: Run them and watch them fail**

```bash
pnpm --filter @loams/desktop exec vitest run test/menu-model.test.ts test/context-menu.test.ts
pnpm --filter @loams/platform-electron test
```

Expected: FAIL. The menu labels lack the App items, there is `no item palette`, `Cannot find module '../src/main/shell/context-menu-template'`, and the platform test sees `[]`.

- [ ] **Step 3: The menu template**

In `menu-model.ts`, `MENU_COMMANDS` becomes:

```ts
export const MENU_COMMANDS: Readonly<Record<string, ShellCommand>> = {
	"mode-studio": "mode.studio",
	"mode-console": "mode.console",
	"mode-toggle": "mode.toggle",
	"new-thread": "chat.new",
	palette: "palette.toggle",
	"toggle-sidebar": "sidebar.toggle",
	"toggle-inspector": "inspector.toggle",
	"toggle-drawer": "drawer.toggle",
	back: "navigation.back",
	forward: "navigation.forward",
	"next-thread": "thread.next",
	"reopen-thread": "thread.reopen",
	settings: "settings.open",
};

export const ISSUES_URL = "https://github.com/ostrium-labs/loams/issues/new/choose";
```

`menuModel` builds, with `const r = (id: string, label: string, accelerator: string): MenuNode => ({ id, label, accelerator, registerAccelerator: false });`:

```ts
export function menuModel(o: { platform: string; dev: boolean }): MenuNode[] {
	const sep: MenuNode = { type: "separator" };
	const mac = o.platform === "darwin";
	const settings = r("settings", mac ? "Settings…" : "Settings", "CmdOrCtrl+,");
	const checkUpdates: MenuNode = { id: "check-updates", label: "Check for Updates…" };
	const menus: MenuNode[] = [];
	if (mac) {
		menus.push({
			label: "Loams Desktop",
			submenu: [{ id: "about", label: "About Loams Desktop" }, checkUpdates, sep, settings, sep,
				{ role: "hide" }, { role: "hideOthers" }, { role: "unhide" }, sep, { id: "quit", label: "Quit Loams Desktop" }],
		});
		menus.push({ label: "File", submenu: [r("new-thread", "New Thread", "CmdOrCtrl+N"), sep, { role: "close" }] });
	} else {
		menus.push({ label: "File", submenu: [r("new-thread", "New Thread", "CmdOrCtrl+N"), sep, settings, sep, { id: "quit", label: "Quit" }] });
	}
	menus.push({
		label: "Edit",
		submenu: [{ role: "undo" }, { role: "redo" }, sep, { role: "cut" }, { role: "copy" }, { role: "paste" }, { role: "selectAll" }],
	});
	const view: MenuNode[] = [
		r("palette", "Command Palette", "CmdOrCtrl+K"),
		sep,
		r("toggle-sidebar", "Toggle Sidebar", "CmdOrCtrl+B"),
		r("toggle-inspector", "Toggle Inspector", "CmdOrCtrl+I"),
		r("toggle-drawer", "Toggle Drawer", "CmdOrCtrl+J"),
		sep,
		r("back", "Back", "CmdOrCtrl+["),
		r("forward", "Forward", "CmdOrCtrl+]"),
		sep,
		{ id: "mode-studio", label: "Studio" },
		{ id: "mode-console", label: "Console" },
		r("mode-toggle", "Switch Mode", "CmdOrCtrl+Shift+M"),
		sep,
		{ role: "resetZoom" },
		{ role: "zoomIn" },
		{ role: "zoomOut" },
		sep,
		{ role: "togglefullscreen" },
	];
	if (o.dev) view.unshift({ role: "reload" }, { role: "toggleDevTools" }, sep);
	menus.push({ label: "View", submenu: view });
	menus.push({
		label: "Window",
		submenu: [{ role: "minimize" }, { role: "zoom" }, sep, r("next-thread", "Next Thread", "Ctrl+Tab"),
			r("reopen-thread", "Reopen Closed Thread", "CmdOrCtrl+Shift+T"), sep, { role: "close" }],
	});
	menus.push({
		label: "Help",
		submenu: [
			{ id: "docs", label: "Documentation" },
			{ id: "show-logs", label: "Show Logs" },
			{ id: "restart-server", label: "Restart Local Server" },
			{ id: "report-issue", label: "Report an Issue…" },
			...(mac ? [] : [sep, checkUpdates, { id: "about", label: "About Loams Desktop" }]),
		],
	});
	return menus;
}
```

`menuCommand` is unchanged. Extend `ShellCommand` in `contracts.ts` with the nine new names. In `index.ts`'s `onAction`, handle the main-only ids:

```ts
					} else if (id === "check-updates") void updater?.check();
					else if (id === "restart-server") void engine?.stop().then(() => engine?.start());
					else if (id === "show-logs") openLogs();
					else if (id === "report-issue") void shell.openExternal(ISSUES_URL);
```

Keep `"logs"` as an alias of `show-logs` for one release. `startUpdater` returns `check: () => ctl.check()` as well; Task 16 makes it a manual check.

- [ ] **Step 4: The context menu**

Copy dsh-desktop's `src/main/context-menu-template.ts` to `src/main/shell/context-menu-template.ts`, then change four things:
- add the header `// Adapted from dataelement/dsh-desktop (MIT), src/main/context-menu-template.ts.`;
- remove the `zh` labels and the `locale` parameter (Loams is English-only), and keep the `en` labels;
- add `lookUp: string` to the labels as `` (text: string) => `Look Up “${text}”` `` and `lookUp: () => void` to `ContextMenuActions`;
- take `platform: NodeJS.Platform` as the second argument, and prepend a Look Up section when `platform === "darwin" && hasSelection`:

```ts
	if (platform === "darwin" && hasSelection) {
		appendSection(template, [{ label: `Look Up “${state.selectionText.trim().slice(0, 40)}”`, click: actions.lookUp }]);
	}
```

`src/main/shell/context-menu.electron.ts`, adapted from dsh-desktop's `src/main/context-menu.ts`:

```ts
// Adapted from dataelement/dsh-desktop (MIT), src/main/context-menu.ts.
import { type BrowserWindow, clipboard, Menu, shell } from "electron";
import { buildContextMenuTemplate } from "./context-menu-template";

export function installContextMenu(window: BrowserWindow): void {
	window.webContents.on("context-menu", (_event, params) => {
		const template = buildContextMenuTemplate(params, process.platform, {
			openLink: (url) => void shell.openExternal(url),
			copyLink: (url) => clipboard.writeText(url),
			copyImage: () => {
				if (!window.isDestroyed()) window.webContents.copyImageAt(params.x, params.y);
			},
			lookUp: () => {
				if (!window.isDestroyed()) window.webContents.showDefinitionForSelection();
			},
		});
		if (template.length === 0 || window.isDestroyed()) return;
		Menu.buildFromTemplate(template).popup({ window });
	});
}
```

Call `installContextMenu(win)` in `createWindow` after `installWindowChrome`. Pages that open their own context menu (Studio's thread rows call `preventDefault()` on `contextmenu`) keep theirs, because Chromium does not raise `context-menu` for a cancelled event.

- [ ] **Step 5: The renderer side**

In `platform-electron`'s `wireShellCommands`, add a branch for the shell's commands:

```ts
      case 'palette.toggle':
      case 'sidebar.toggle':
      case 'inspector.toggle':
      case 'drawer.toggle':
      case 'navigation.back':
      case 'navigation.forward':
      case 'settings.open':
      case 'thread.next':
      case 'thread.reopen':
        target.dispatchEvent(new CustomEvent(SHELL_COMMAND_EVENT, { detail: command, cancelable: true }));
        return;
```

In `web/plugins/shell/src/keyboard.tsx`, move the `switch` into `function run(command: ShellKeyCommand): boolean` (true when handled). `onKey` then calls `if (run(command)) e.preventDefault()`. Add a second listener for menu commands:

```tsx
  useEffect(() => {
    const onCommand = (event: Event) => {
      const detail = (event as CustomEvent<unknown>).detail;
      if (typeof detail !== 'string' || detail === 'thread.next' || detail === 'chat.new') return; // Studio's
      if (SHELL_COMMANDS.has(detail as ShellKeyCommand) && run(detail as ShellKeyCommand)) event.preventDefault();
    };
    window.addEventListener(SHELL_COMMAND_EVENT, onCommand);
    return () => window.removeEventListener(SHELL_COMMAND_EVENT, onCommand);
  }, []);
```

`SHELL_COMMANDS` is `new Set(SHELL_KEYMAP.map((b) => b.command))`.

- [ ] **Step 6: Run the tests**

```bash
pnpm --filter @loams/desktop test
pnpm --filter @loams/platform-electron test
pnpm --filter @loams/plugin-shell test
pnpm --filter @loams/desktop typecheck
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop-electron web/packages/platform-electron web/plugins/shell
git commit -s -m "feat(desktop): native menus with the D1081 accelerators and a context menu (CF1 task 14)"
```

---

### Task 15: The splash, the loader generator and dsh-desktop provenance

**Files:**
- Create: `apps/desktop-electron/build/splash.html` (adapted from dsh-desktop `build/splash.html`)
- Create: `apps/desktop-electron/scripts/generate-loader-gifs.mjs` (adapted from dsh-desktop `scripts/generate-loader-gifs.mjs`, W14)
- Create (generated): `apps/desktop-electron/build/loams-loader.gif`, `build/loams-loader-dark.gif`
- Create: `apps/desktop-electron/src/main/shell/splash.ts` (pure `SplashGate`), `src/main/shell/splash.electron.ts` (the window)
- Modify: `apps/desktop-electron/src/main/index.ts` (show the splash; show the main window on ready), `src/main/shell/shell-ipc.electron.ts` (`shell:ready`), `src/preload/index.ts`, `src/shared/contracts.ts` (`CH.shellReady`, `shell.ready()`)
- Modify: `web/apps/console/src/cordis/desktop.ts:86-104` (report ready), `web/apps/console/src/cordis/fake-desktop.ts` (`ready`)
- Create: `apps/desktop-electron/provenance.json`, `apps/desktop-electron/LICENSE.dsh-desktop`
- Modify: `apps/desktop-electron/NOTICE`, the root `NOTICE`, `apps/desktop-electron/electron-builder.config.cjs:49-60`, `apps/desktop-electron/package.json` (`gifenc`, `loader:generate`)
- Create: `apps/desktop-electron/test/provenance.test.ts`, `apps/desktop-electron/test/splash.test.ts`, `apps/desktop-electron/test/e2e/windows.ts` (`mainWindow`)
- Modify: `apps/desktop-electron/test/bundle-notice.test.ts`; every `app.firstWindow()` in `test/e2e/*.spec.ts`

**Interfaces:**
- Consumes: `CHROME` (Task 13), Loams' mark geometry (`web/packages/ui/src/logo.tsx:31-36`).
- Produces:
  - `SplashGate` (`new SplashGate({ timeoutMs, onShow, schedule })` with `ready()` and `dispose()`) and `SPLASH_TIMEOUT_MS = 30_000`.
  - `showSplash(o: { dark: boolean }): { close(): void }` and `splashPath(): string`.
  - `CH.shellReady` and `LoamsDesktopApi.shell.ready(): Promise<void>`.
  - `mainWindow(app: ElectronApplication): Promise<Page>` (test helper).
  - `provenance.json` with the row shape of `web/packages/ui/provenance.json`.

- [ ] **Step 1: Write the failing tests**

`apps/desktop-electron/test/splash.test.ts`:

```ts
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { SPLASH_TIMEOUT_MS, SplashGate } from "../src/main/shell/splash";

const build = resolve(__dirname, "../build");

describe("the splash (D1084)", () => {
	it("shows the main window once, on ready or after the timeout", () => {
		let fire: () => void = () => {};
		const onShow = vi.fn();
		const gate = new SplashGate({ timeoutMs: SPLASH_TIMEOUT_MS, onShow, schedule: (fn) => ((fire = fn), () => {}) });
		gate.ready();
		gate.ready();
		fire();
		expect(onShow).toHaveBeenCalledOnce();
		const late = vi.fn();
		const g2 = new SplashGate({ timeoutMs: 10, onShow: late, schedule: (fn) => ((fire = fn), () => {}) });
		fire();
		g2.ready();
		expect(late).toHaveBeenCalledOnce();
	});

	it("is a local page with a strict CSP and both loaders", () => {
		const html = readFileSync(resolve(build, "splash.html"), "utf8");
		expect(html).toContain(`http-equiv="Content-Security-Policy"`);
		expect(html).toContain("default-src 'none'");
		expect(html).not.toMatch(/https?:\/\//);
		expect(html).toContain("Starting Loams Desktop");
		for (const gif of ["loams-loader.gif", "loams-loader-dark.gif"]) {
			expect(html).toContain(gif);
			expect(existsSync(resolve(build, gif))).toBe(true);
			expect(readFileSync(resolve(build, gif)).subarray(0, 6).toString()).toBe("GIF89a");
		}
	});
});
```

`apps/desktop-electron/test/provenance.test.ts`:

```ts
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const app = resolve(__dirname, "..");
const HEADER = /^\/\/ Adapted from dataelement\/dsh-desktop \(MIT\), (\S+)\.$|^<!-- Adapted from dataelement\/dsh-desktop \(MIT\), (\S+)\. -->$/m;
const prov = JSON.parse(readFileSync(join(app, "provenance.json"), "utf8")) as {
	upstream: string;
	license: string;
	copyright: string;
	files: { path: string; upstreamPath: string; commit: string; upstreamSha256: string; localSha256: string; verdict: string }[];
};

function* files(dir: string): Generator<string> {
	for (const name of readdirSync(dir)) {
		if (["node_modules", "out", "dist", "test-results"].includes(name) || name.startsWith(".")) continue;
		const p = join(dir, name);
		if (statSync(p).isDirectory()) yield* files(p);
		else if (/\.(ts|mjs|cjs|html)$/.test(name)) yield p;
	}
}

describe("dsh-desktop provenance (§10)", () => {
	it("names dsh-desktop, its licence and copyright", () => {
		expect(prov.upstream).toBe("https://github.com/dataelement/dsh-desktop");
		expect(prov.license).toBe("MIT");
		expect(prov.copyright).toBe("Copyright (c) 2026 DataElement");
		expect(readFileSync(join(app, "LICENSE.dsh-desktop"), "utf8")).toMatch(/MIT License[\s\S]*Copyright \(c\) 2026 DataElement/);
		const notice = readFileSync(join(app, "NOTICE"), "utf8");
		expect(notice).toContain("LICENSE.dsh-desktop");
		expect(notice).toContain("provenance.json");
	});

	it("has a row for every adapted file, and every row's file says where it came from", () => {
		const rows = new Map(prov.files.map((r) => [r.path, r]));
		const headed: string[] = [];
		for (const p of [...files(join(app, "src")), ...files(join(app, "scripts")), ...files(join(app, "build")), join(app, "electron.vite.config.ts")]) {
			const m = HEADER.exec(readFileSync(p, "utf8"));
			if (!m) continue;
			const rel = relative(app, p);
			headed.push(rel);
			expect(rows.get(rel)?.upstreamPath, rel).toBe(m[1] ?? m[2]);
		}
		expect([...rows.keys()].sort()).toEqual(headed.sort());
		for (const r of prov.files) {
			expect(r.commit).toBe("bc732f5dc4c5465915d5b3f28d9b0b32703fc664");
			expect(r.upstreamSha256).toMatch(/^[0-9a-f]{64}$/);
			expect(["as-is", "adapt", "rewrite"]).toContain(r.verdict);
			if (r.verdict === "as-is") expect(createHash("sha256").update(readFileSync(join(app, r.path))).digest("hex")).toBe(r.localSha256);
		}
	});

	it("never depends on @deepseek-ai packages", () => {
		const manifest = readFileSync(join(app, "package.json"), "utf8");
		expect(manifest).not.toContain("@deepseek-ai");
		for (const p of files(join(app, "src"))) expect(readFileSync(p, "utf8"), p).not.toContain("@deepseek-ai");
	});
});
```

In `test/bundle-notice.test.ts`, add:

```ts
	it("ships the dsh-desktop licence and the splash", () => {
		const shipped = new Map(config.extraResources.map((r) => [r.to, resolve(app, r.from)]));
		for (const to of ["LICENSE.dsh-desktop", "splash/splash.html", "splash/loams-loader.gif", "splash/loams-loader-dark.gif"]) {
			expect(existsSync(shipped.get(to) ?? ""), to).toBe(true);
		}
	});
```

- [ ] **Step 2: Run them and watch them fail**

Run: `pnpm --filter @loams/desktop exec vitest run test/splash.test.ts test/provenance.test.ts test/bundle-notice.test.ts`
Expected: FAIL, with `Cannot find module '../src/main/shell/splash'`, `ENOENT … provenance.json` and `LICENSE.dsh-desktop: expected false to be true`.

- [ ] **Step 3: The generator and the GIFs (W14)**

Add `"gifenc": "1.0.3"` to `apps/desktop-electron/package.json` devDependencies (MIT, 2021), and `"loader:generate": "node scripts/generate-loader-gifs.mjs"` to its scripts. Run `pnpm install`. Then write `scripts/generate-loader-gifs.mjs`: dsh-desktop's script with the mark drawn from rectangles and `gifenc` in place of `sharp`.

```js
// Adapted from dataelement/dsh-desktop (MIT), scripts/generate-loader-gifs.mjs.
// Regenerate the splash loaders (build/loams-loader.gif, build/loams-loader-dark.gif) from
// Loams' mark (web/packages/ui/src/logo.tsx: an L of strata with an indigo seed). Upstream
// rasterises an SVG with sharp; Loams' mark is rectangles, so it is drawn on the cell grid
// directly and encoded with gifenc (MIT): no native image library (W14). The mark bobs
// gently and sheds a few square "bubbles" from its seed, as upstream's loader does.
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GIFEncoder } from 'gifenc'

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const buildDirectory = path.join(projectRoot, 'build')

const WIDTH = 320
const HEIGHT = 240
const FRAMES = 48
const DELAY_MS = 50
const CELL = 4
const COLS = WIDTH / CELL
const ROWS = HEIGHT / CELL
/** Cells per unit of the mark's 26-unit view box (viewBox="3 3 26 26"). */
const SCALE = 2

/** logo.tsx's full cut: [x, y, width, height] in view-box units; the last is the seed. */
const STRATA = [
  [3, 3, 6, 26],
  [3, 23, 26, 6],
  [12, 16, 14, 4],
  [12, 10, 9, 3],
]
const SEED = [12, 3, 4, 4]

/** Palette index 0 is transparent; 1 the body; 2 the seed; 3–5 the bubbles, fading. */
const THEMES = {
  light: { file: 'loams-loader.gif', palette: [[0, 0, 0], [29, 27, 24], [27, 78, 216], [91, 128, 230], [150, 176, 240], [205, 216, 248]] },
  dark: { file: 'loams-loader-dark.gif', palette: [[0, 0, 0], [245, 245, 245], [119, 162, 252], [86, 120, 214], [56, 82, 160], [34, 48, 96]] },
}

function rng(seed) {
  let state = seed >>> 0
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0
    return state / 0x100000000
  }
}

function markCells() {
  const cells = []
  const add = ([x, y, w, h], colour) => {
    for (let cy = (y - 3) * SCALE; cy < (y - 3 + h) * SCALE; cy += 1) {
      for (let cx = (x - 3) * SCALE; cx < (x - 3 + w) * SCALE; cx += 1) cells.push([cx, cy, colour])
    }
  }
  for (const r of STRATA) add(r, 1)
  add(SEED, 2)
  return cells
}

function buildSparks(origin, random) {
  const sparks = []
  for (let index = 0; index < 8; index += 1) {
    sparks.push({
      start: Math.floor(random() * FRAMES),
      life: 14 + Math.floor(random() * 8),
      x: origin.x + Math.floor(random() * 6) - 1,
      y: origin.y,
      drift: random() < 0.5 ? -1 : 1,
      size: random() < 0.35 ? 1 : 2,
    })
  }
  return sparks
}

function renderFrame(index, mark, sparks, body) {
  const pixels = new Uint8Array(WIDTH * HEIGHT)
  const fillCell = (cx, cy, size, colour) => {
    for (let dy = 0; dy < size; dy += 1) {
      for (let dx = 0; dx < size; dx += 1) {
        const x = cx + dx
        const y = cy + dy
        if (x < 0 || y < 0 || x >= COLS || y >= ROWS) continue
        for (let py = 0; py < CELL; py += 1) pixels.fill(colour, (y * CELL + py) * WIDTH + x * CELL, (y * CELL + py) * WIDTH + x * CELL + CELL)
      }
    }
  }
  const phase = (index / FRAMES) * Math.PI * 2
  const bobY = Math.round(Math.sin(phase) * 2)
  const bobX = Math.round(Math.cos(phase))
  for (const [cx, cy, colour] of mark) fillCell(body.left + cx + bobX, body.top + cy + bobY, 1, colour)
  for (const spark of sparks) {
    const age = (index - spark.start + FRAMES) % FRAMES
    if (age >= spark.life) continue
    const progress = age / spark.life
    const rise = Math.floor(age / 2)
    const sway = Math.round(Math.sin(progress * Math.PI * 2) * spark.drift)
    const shade = 3 + Math.min(2, Math.floor(progress * 3))
    fillCell(spark.x + sway + bobX, spark.y - rise + bobY, progress > 0.75 ? 1 : spark.size, shade)
  }
  return pixels
}

const mark = markCells()
const size = 26 * SCALE
const body = { left: Math.round((COLS - size) / 2), top: Math.round((ROWS - size) / 2) + 2 }
const sparks = buildSparks({ x: body.left + (SEED[0] - 3) * SCALE + 2, y: body.top - 1 }, rng(0x5a17))
for (const theme of Object.values(THEMES)) {
  const gif = GIFEncoder()
  for (let index = 0; index < FRAMES; index += 1) {
    gif.writeFrame(renderFrame(index, mark, sparks, body), WIDTH, HEIGHT, {
      palette: theme.palette,
      delay: DELAY_MS,
      transparent: true,
      transparentIndex: 0,
      repeat: 0,
    })
  }
  gif.finish()
  const destination = path.join(buildDirectory, theme.file)
  await writeFile(destination, gif.bytes())
  console.log(`Wrote ${path.relative(projectRoot, destination)} (${gif.bytes().length} bytes, ${FRAMES} frames)`)
}
```

The palette RGB triples are the accent and ink tokens' sRGB values. The colour test scans `web/` only, and this script is under `apps/`. Run:

```bash
pnpm --filter @loams/desktop loader:generate
```

Expected: `Wrote build/loams-loader.gif (… bytes, 48 frames)` and the dark one. Commit both GIFs. The rng is fixed, so a regeneration is byte-identical.

- [ ] **Step 4: The splash page and window**

`apps/desktop-electron/build/splash.html`, adapted from dsh-desktop's `build/splash.html`. The theme comes from `prefers-color-scheme`, which follows `nativeTheme.themeSource` (Task 2), so the page needs no script:

```html
<!doctype html>
<!-- Adapted from dataelement/dsh-desktop (MIT), build/splash.html. -->
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self'; style-src 'unsafe-inline'" />
    <meta name="viewport" content="width=device-width,initial-scale=1" />
    <title>Loams Desktop</title>
    <style>
      :root { color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, sans-serif; }
      * { box-sizing: border-box; }
      html, body { width: 100%; height: 100%; margin: 0; overflow: hidden; -webkit-app-region: drag; user-select: none; }
      body { color: #1d1b18; background: #ffffff; }
      main { position: fixed; inset: 0; display: grid; align-content: center; justify-items: center; gap: 10px; padding: 24px; text-align: center; }
      .mark { width: 160px; height: auto; image-rendering: pixelated; }
      .dark { display: none; }
      h1 { margin: 0; font-size: 16px; font-weight: 600; letter-spacing: -0.01em; }
      p { margin: 0; color: #6b665e; font-size: 13px; }
      @media (prefers-color-scheme: dark) {
        body { color: #f5f5f5; background: #0a0a0a; }
        .light { display: none; }
        .dark { display: block; }
        p { color: #a3a3a3; }
      }
    </style>
  </head>
  <body>
    <main>
      <img class="mark light" src="loams-loader.gif" alt="" />
      <img class="mark dark" src="loams-loader-dark.gif" alt="" />
      <h1>Starting Loams Desktop</h1>
      <p>The local engine is starting. The first launch may take a moment.</p>
    </main>
  </body>
</html>
```

`src/main/shell/splash.ts`:

```ts
// The splash gate (D1084): the main window shows when the console reports ready, or after
// SPLASH_TIMEOUT_MS so a slow or failed start still shows the console's own boot screen.

export const SPLASH_TIMEOUT_MS = 30_000;

export class SplashGate {
	#done = false;
	readonly #cancel: () => void;

	constructor(
		private readonly o: {
			timeoutMs: number;
			onShow: () => void;
			schedule?: (fn: () => void, ms: number) => () => void;
		},
	) {
		const schedule =
			o.schedule ??
			((fn: () => void, ms: number) => {
				const t = setTimeout(fn, ms);
				return () => clearTimeout(t);
			});
		this.#cancel = schedule(() => this.#show(), o.timeoutMs);
	}

	ready(): void {
		this.#show();
	}

	dispose(): void {
		this.#cancel();
	}

	#show(): void {
		if (this.#done) return;
		this.#done = true;
		this.#cancel();
		this.o.onShow();
	}
}
```

`src/main/shell/splash.electron.ts`:

```ts
import { join } from "node:path";
import { app, BrowserWindow } from "electron";
import { CHROME } from "./window-chrome";

export function splashPath(): string {
	return app.isPackaged ? join(process.resourcesPath, "splash", "splash.html") : join(app.getAppPath(), "build", "splash.html");
}

/** A frameless splash while the local stack starts (D1084; T3 Code's connecting splash window). */
export function showSplash(o: { dark: boolean }): { close(): void } {
	const splash = new BrowserWindow({
		width: 360,
		height: 280,
		frame: false,
		resizable: false,
		minimizable: false,
		maximizable: false,
		fullscreenable: false,
		center: true,
		show: false,
		backgroundColor: o.dark ? CHROME.dark.canvas : CHROME.light.canvas,
		title: "Loams Desktop",
		webPreferences: { contextIsolation: true, nodeIntegration: false, sandbox: true },
	});
	splash.once("ready-to-show", () => {
		if (!splash.isDestroyed()) splash.show();
	});
	void splash.loadFile(splashPath());
	return {
		close: () => {
			if (!splash.isDestroyed()) splash.close();
		},
	};
}
```

In `index.ts`:

1. In `app.whenReady`, right after `const open = …` (index.ts:323-327) and before `registerShellIpc`, show the splash and create the gate. The first console window is still created by `open()` at index.ts:413, hidden until the gate shows it.

```ts
			const splash = showSplash({ dark: sync.dark });
			const gate = new SplashGate({
				timeoutMs: SPLASH_TIMEOUT_MS,
				onShow: () => {
					const w = getMainWindow();
					if (w && !w.isDestroyed()) w.show();
					splash.close();
				},
			});
```

2. Pass `onReady: () => gate.ready()` to `registerShellIpc`.
3. In `createWindow`, delete `win.once("ready-to-show", () => win.show())`. A window created later (from the tray, after the gate has shown the first) keeps a `ready-to-show` handler that shows it when `gateShown` is true. Keep a module-level `let gateShown = false`, set it in `onShow`, and add `win.once("ready-to-show", () => { if (gateShown) win.show(); })`.

In `shell-ipc.electron.ts`, extend `ShellIpcDeps` with `onReady: () => void`, and add:

```ts
	ipcMain.handle(CH.shellReady, (e) => {
		assertTrustedSender(e);
		deps.onReady();
	});
```

Add `CH.shellReady = "shell:ready"` and `ready(): Promise<void>` to the `shell` API, and `ready: () => invoke(CH.shellReady)` to the preload. In `web/apps/console/src/cordis/desktop.ts`'s `startDesktop`, after `await wireDeepLinks(api)`:

```ts
  // The splash closes once the console has painted (D1084).
  requestAnimationFrame(() => void api.shell.ready?.());
```

`fake-desktop.ts` adds `ready: async () => {}`.

- [ ] **Step 5: Provenance, licence and NOTICE**

```bash
cd apps/desktop-electron
cp ~/Documents/research/dsh-desktop/LICENSE LICENSE.dsh-desktop
for f in src/main/runtime/harness-runtime.ts src/main/close-to-tray.ts src/main/security.ts src/main/security-policy.ts \
         src/main/state/window-state.ts src/main/update/update-manager.ts electron.vite.config.ts src/main/index.ts \
         src/main/macos-window-backdrop.ts src/main/context-menu-template.ts src/main/context-menu.ts \
         src/preload/macos-window-chrome.ts build/splash.html scripts/generate-loader-gifs.mjs; do
  printf '%s %s\n' "$f" "$(sha256sum ~/Documents/research/dsh-desktop/$f | cut -d' ' -f1)"
done
```

Write `provenance.json` with one row per adapted local file. Use the upstream sums printed above, and compute each `localSha256` with `sha256sum <local path>`:

```json
{
  "upstream": "https://github.com/dataelement/dsh-desktop",
  "license": "MIT",
  "copyright": "Copyright (c) 2026 DataElement",
  "files": [
    { "path": "electron.vite.config.ts", "upstreamPath": "electron.vite.config.ts", "commit": "bc732f5dc4c5465915d5b3f28d9b0b32703fc664", "upstreamSha256": "<from the loop>", "localSha256": "<sha256sum>", "verdict": "adapt", "note": "adapted before CF1 (W19)" },
    { "path": "src/main/engine/ports.ts", "upstreamPath": "src/main/runtime/harness-runtime.ts", "commit": "bc732f5dc4c5465915d5b3f28d9b0b32703fc664", "upstreamSha256": "<from the loop>", "localSha256": "<sha256sum>", "verdict": "adapt", "note": "adapted before CF1 (W19)" }
  ]
}
```

The full set of rows, as `local path ← upstream path`:
- Adapted before CF1 (W19): `src/main/engine/ports.ts ← src/main/runtime/harness-runtime.ts`, `src/main/engine/supervisor.ts ← src/main/runtime/harness-runtime.ts`, `src/main/shell/tray.electron.ts ← src/main/close-to-tray.ts`, `src/main/security/install.electron.ts ← src/main/security.ts`, `src/main/security/policy.ts ← src/main/security-policy.ts`, `src/main/shell/window-state.ts ← src/main/state/window-state.ts` and `src/main/update/updater.electron.ts ← src/main/update/update-manager.ts`, plus `electron.vite.config.ts`.
- New in CF1: `src/main/shell/window-chrome.ts ← src/main/index.ts`, `src/main/shell/macos-window-backdrop.ts ← src/main/macos-window-backdrop.ts`, `src/main/shell/context-menu-template.ts ← src/main/context-menu-template.ts`, `src/main/shell/context-menu.electron.ts ← src/main/context-menu.ts`, `src/preload/window-chrome.ts ← src/preload/macos-window-chrome.ts`, `build/splash.html ← build/splash.html` and `scripts/generate-loader-gifs.mjs ← scripts/generate-loader-gifs.mjs`. Task 16 adds `src/preload/notification-shim.ts ← src/preload/desktop-notification.ts`.

Each row's `upstreamPath` must equal the path in that file's header line, which the test checks. In `NOTICE`, after the dsh-desktop paragraph, add:

```
The dsh-desktop files are listed, with their upstream paths, commit
bc732f5dc4c5465915d5b3f28d9b0b32703fc664 and SHA-256 sums, in this app's
provenance.json. dsh-desktop's MIT licence text is LICENSE.dsh-desktop and ships
with the app. Only the dataelement/dsh-desktop repository's own source is used;
the prebuilt @deepseek-ai/* packages it loads are not copied, read or bundled.
```

In the root `NOTICE`'s third-party list, add: "dsh-desktop (https://github.com/dataelement/dsh-desktop), MIT License, Copyright (c) 2026 DataElement. Files under apps/desktop-electron/ that say "Adapted from dataelement/dsh-desktop" derive from it; apps/desktop-electron/provenance.json lists them and LICENSE.dsh-desktop carries the licence." In `electron-builder.config.cjs`'s `extraResources`, add:

```js
		{ from: "LICENSE.dsh-desktop", to: "LICENSE.dsh-desktop" },
		{ from: "build/splash.html", to: "splash/splash.html" },
		{ from: "build/loams-loader.gif", to: "splash/loams-loader.gif" },
		{ from: "build/loams-loader-dark.gif", to: "splash/loams-loader-dark.gif" },
```

- [ ] **Step 6: The e2e specs find the main window, not the splash**

`apps/desktop-electron/test/e2e/windows.ts`:

```ts
import type { ElectronApplication, Page } from "@playwright/test";

/** The console window (the splash opens first since CF1 Task 15). */
export async function mainWindow(app: ElectronApplication): Promise<Page> {
	for (let i = 0; i < 200; i++) {
		const page = app.windows().find((w) => w.url().startsWith("loams-app://console"));
		if (page) return page;
		await new Promise((r) => setTimeout(r, 100));
	}
	throw new Error("the console window did not open");
}
```

Replace `await app.firstWindow()` with `await mainWindow(app)` in `cookies.spec.ts`, `smoke.spec.ts` and `studio.spec.ts`.

- [ ] **Step 7: Run the tests**

```bash
pnpm --filter @loams/desktop test
node --test scripts/ci/npm-licences.test.mjs
pnpm --filter @loams/console build && pnpm --filter @loams/desktop build
LOAMS_E2E_ELECTRON_ARGS=--no-sandbox xvfb-run -a pnpm --filter @loams/desktop test:e2e cookies
```

Expected: PASS. The cookies spec opens the console window behind the splash.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop-electron web/apps/console/src/cordis NOTICE pnpm-lock.yaml
git commit -s -m "feat(desktop): a splash with Loams' loader, and dsh-desktop provenance (CF1 task 15)"
```

---

### Task 16: OS notifications and the update toast

**Files:**
- Create: `apps/desktop-electron/src/preload/notification-shim.ts` (adapted from dsh-desktop `src/preload/desktop-notification.ts`)
- Modify: `apps/desktop-electron/src/preload/index.ts` (install the shim; `update.onState`, `update.skip`), `src/shared/contracts.ts` (`UpdateState`, `CH.updateSkip`, the API), `src/main/settings.ts` (`UPDATE_SKIPPED_KEY`)
- Modify: `apps/desktop-electron/src/main/update/controller.ts` (`check({ manual })`, `skip`), `src/main/update/updater.electron.ts` (`update:skip`; scheduler checks are automatic; `check` is manual)
- Modify: `apps/desktop-electron/provenance.json` (the shim's row)
- Modify: `web/packages/platform-web/src/index.ts:81-85` (click opens the route)
- Modify: `web/plugins/studio/src/notifications/data.ts` (`onArrive`), `src/header.tsx` (web OS notifications), `src/index.tsx`
- Create: `web/plugins/desktop-settings/src/update-toast.tsx`; modify `src/index.tsx` (register it in `shell.overlay`)
- Modify: `web/apps/console/src/cordis/fake-desktop.ts` (`update.onState`, `update.skip`)
- Create: `apps/desktop-electron/test/update-skip.test.ts`, `web/plugins/desktop-settings/test/update-toast.test.tsx`
- Modify: `web/plugins/studio/test/notifications.test.tsx`

**Interfaces:**
- Consumes: `toastManager` and `ToastOptions` (Task 6); the existing update controller, `shell:notify`, `studioWatch` and `shouldNotify` (W15).
- Produces:
  - `UpdateState`; `CH.updateSkip`; `LoamsDesktopApi.update.onState(cb)` and `update.skip(version)`; `UPDATE_SKIPPED_KEY = "update.skippedVersion"`.
  - `Controller.check(o?: { manual?: boolean }): Promise<void>`, `Controller.skip(version: string): void` and `ControllerDeps.skipped(): string | undefined` / `saveSkipped(version: string): void`.
  - `UpdateToast({ update }: { update: LoamsDesktopApi['update'] })` and `installNotificationShim(): void`.
  - `useNotifications(onArrive?: (n: StudioNotification) => void)`.

- [ ] **Step 1: Write the failing tests**

`apps/desktop-electron/test/update-skip.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { type ControllerDeps, createController } from "../src/main/update/controller";

/** A signed manifest's content as parsePinned reads it (pin.ts:27-51). */
const YML = [
	"version: 0.2.0",
	"files:",
	"  - url: Loams-Desktop-0.2.0.AppImage",
	"    sha512: abc",
	"path: Loams-Desktop-0.2.0.AppImage",
	"sha512: abc",
	"",
].join("\n");
const INFO = {
	version: "0.2.0",
	files: [{ url: "Loams-Desktop-0.2.0.AppImage", sha512: "abc" }],
	path: "Loams-Desktop-0.2.0.AppImage",
	sha512: "abc",
};

function deps(over: Partial<ControllerDeps> = {}): ControllerDeps & { states: string[] } {
	const states: string[] = [];
	let skipped: string | undefined;
	return {
		states,
		mode: "manual",
		appVersion: "0.1.0",
		updater: { autoDownload: false, autoInstallOnAppQuit: false, checkForUpdates: vi.fn(), downloadUpdate: vi.fn(), quitAndInstall: vi.fn(), on: vi.fn() },
		fetchFeed: async () => ({ yml: new TextEncoder().encode(YML), sig: new Uint8Array() }),
		verify: async () => true,
		hashFile: async () => "",
		deleteFile: async () => {},
		openRelease: async () => {},
		onState: (s) => void states.push(`${s.phase}${s.version ? `:${s.version}` : ""}`),
		skipped: () => skipped,
		saveSkipped: (v) => {
			skipped = v;
		},
		...over,
	} as ControllerDeps & { states: string[] };
}

describe("skip this version (D1085)", () => {
	it("an automatic check stays quiet about a skipped version; a manual check offers it again", async () => {
		const d = deps();
		const ctl = createController(d);
		await ctl.check({ manual: false });
		expect(ctl.state()).toMatchObject({ phase: "available", version: "0.2.0" });
		ctl.skip("0.2.0");
		expect(ctl.state().phase).toBe("idle");
		await ctl.check({ manual: false });
		expect(ctl.state().phase).toBe("idle");
		await ctl.check({ manual: true });
		expect(ctl.state()).toMatchObject({ phase: "available", version: "0.2.0" });
	});

	it("never downloads before consent", async () => {
		const d = deps({ mode: "self" });
		(d.updater.checkForUpdates as ReturnType<typeof vi.fn>).mockResolvedValue({ isUpdateAvailable: true, updateInfo: INFO });
		const ctl = createController(d);
		await ctl.check({ manual: false });
		expect(d.updater.downloadUpdate).not.toHaveBeenCalled();
	});
});
```

`web/plugins/desktop-settings/test/update-toast.test.tsx`:

```tsx
import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Toaster, ToastProvider } from '@loams/ui';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { UpdateToast } from '../src/update-toast.js';

afterEach(cleanup);
Element.prototype.getAnimations ??= () => [];

function fakeUpdate() {
  let push: (s: unknown) => void = () => {};
  const update = {
    state: vi.fn(async () => ({ phase: 'idle' as const })),
    onState: (cb: (s: unknown) => void) => ((push = cb), () => {}),
    download: vi.fn(async () => {}),
    installAndRestart: vi.fn(async () => {}),
    skip: vi.fn(async () => {}),
    check: vi.fn(async () => {}),
    autoCheck: vi.fn(async () => ({ enabled: true, locked: false })),
    setAutoCheck: vi.fn(async () => {}),
  } as unknown as LoamsDesktopApi['update'];
  return { update, push: (s: unknown) => act(() => push(s)) };
}

describe('the update toast (D1085)', () => {
  it('asks before downloading, offers to skip, then to restart', async () => {
    const { update, push } = fakeUpdate();
    render(
      <ToastProvider>
        <UpdateToast update={update} />
        <Toaster />
      </ToastProvider>,
    );
    push({ phase: 'available', version: '0.2.0', mode: 'self' });
    expect(await screen.findByText('Loams Desktop 0.2.0 is available')).toBeTruthy();
    expect(update.download).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Download' }));
    expect(update.download).toHaveBeenCalledOnce();
    push({ phase: 'ready', version: '0.2.0' });
    fireEvent.click(await screen.findByRole('button', { name: 'Restart to update' }));
    expect(update.installAndRestart).toHaveBeenCalledOnce();
  });

  it('skips a version', async () => {
    const { update, push } = fakeUpdate();
    render(
      <ToastProvider>
        <UpdateToast update={update} />
        <Toaster />
      </ToastProvider>,
    );
    push({ phase: 'available', version: '0.3.0', mode: 'manual' });
    fireEvent.click(await screen.findByRole('button', { name: 'Skip this version' }));
    expect(update.skip).toHaveBeenCalledWith('0.3.0');
  });
});
```

In `web/plugins/studio/test/notifications.test.tsx` (its `daemon()` fake streams what `post` adds), add:

```tsx
  it('calls onArrive for notifications that arrive after the page opened (W15)', async () => {
    const d = daemon();
    d.post({ id: 'old', kind: NotificationKind.TURN_DONE, title: 'Earlier', createdAtMs: BigInt(Date.now() - 60_000) });
    const arrived: string[] = [];
    render(
      wrap(
        <AgentdScope agentd={d.agentd}>
          <NotificationsBell navigate={vi.fn()} onArrive={(n) => arrived.push(n.id)} />
        </AgentdScope>,
      ),
    );
    d.post({ id: 'n-new', kind: NotificationKind.APPROVAL, title: 'Deploy needs approval', createdAtMs: BigInt(Date.now() + 1000) });
    await waitFor(() => expect(arrived).toEqual(['n-new']));
  });
```

- [ ] **Step 2: Run them and watch them fail**

```bash
pnpm --filter @loams/desktop exec vitest run test/update-skip.test.ts
pnpm --filter @loams/plugin-desktop-settings test
pnpm --filter @loams/plugin-studio exec vitest run test/notifications.test.tsx
```

Expected: FAIL. `ctl.skip is not a function`, `Cannot find module '../src/update-toast.js'`, and `arrived` is `[]`.

- [ ] **Step 3: Skip, and automatic against manual checks**

In `controller.ts`, add to `ControllerDeps`:

```ts
	/** The version the person skipped (`update.skippedVersion`); automatic checks stay quiet about it. */
	skipped?: () => string | undefined;
	saveSkipped?: (version: string) => void;
```

Add `skip(version: string): void` to `Controller`. Change `check()` to `check(o: { manual?: boolean } = {})`. In both branches that set `available`, put this check before them:

```ts
				if (!o.manual && d.skipped?.() === p.version) {
					set({ phase: "idle" });
					return;
				}
```

Then add:

```ts
		skip(version) {
			d.saveSkipped?.(version);
			if (state.phase === "available" && state.version === version) set({ phase: "idle" });
		},
```

In `updater.electron.ts`, pass `skipped: () => readSetting<string | undefined>(opts.settingsFile, UPDATE_SKIPPED_KEY, undefined)` and `saveSkipped: (v) => writeSetting(opts.settingsFile, UPDATE_SKIPPED_KEY, v)`. The scheduler calls `ctl.check({ manual: false })`. `CH.updateCheck` and the returned `check` call `ctl.check({ manual: true })`. Add:

```ts
	ipcMain.handle(CH.updateSkip, (e, version: unknown) => {
		assertTrustedSender(e);
		if (typeof version === "string" && version.length > 0 && version.length < 64) ctl.skip(version);
	});
```

The opt-out (#688) is unchanged: with `update.autoCheck` off, the scheduler never runs, so no automatic check and no toast happen; "Check now" is still a manual check. In `contracts.ts`, add `updateSkip: "update:skip"` to `CH`, and `export type UpdateState = Awaited<ReturnType<LoamsDesktopApi["update"]["state"]>>;`. Add `onState(cb: (s: UpdateState) => void): () => void;` and `skip(version: string): Promise<void>;` to `update`. The preload adds `onState: (cb) => subscribe(CH.updateState, cb)` and `skip: (v) => invoke(CH.updateSkip, v)`. `fake-desktop.ts` adds `onState: () => () => {}` and `skip: async () => {}`.

- [ ] **Step 4: The update toast**

`web/plugins/desktop-settings/src/update-toast.tsx` (dsh-desktop's consent-before-download flow: offer, download on consent, restart; skip a version):

```tsx
import type { LoamsDesktopApi, UpdateState } from '@loams/desktop/contracts';
import { useToast } from '@loams/ui';
import { useEffect, useRef } from 'react';

const ID = 'loams-update';

export function UpdateToast({ update }: { update: LoamsDesktopApi['update'] }) {
  const toast = useToast();
  const shown = useRef<string | null>(null);

  useEffect(() => {
    const onState = (s: UpdateState) => {
      const version = s.version ?? '';
      const show = (o: Parameters<typeof toast.add>[0]) => {
        if (shown.current) toast.update(shown.current, o);
        else shown.current = toast.add({ ...o, id: ID });
      };
      if (s.phase === 'available') {
        show({
          title: `Loams Desktop ${version} is available`,
          description: s.mode === 'manual' ? 'It downloads from the release page.' : 'Download it now; you choose when to restart.',
          type: 'info',
          timeout: 0,
          actionProps: { children: s.mode === 'manual' ? `Download ${version}` : 'Download', onClick: () => void update.download() },
          data: { secondaryActionProps: { children: 'Skip this version', onClick: () => void update.skip(version) } },
        });
      } else if (s.phase === 'downloading') {
        show({ title: `Downloading ${version}`, description: `${Math.round(s.percent ?? 0)}%`, type: 'loading', timeout: 0 });
      } else if (s.phase === 'ready') {
        show({
          title: `Loams Desktop ${version} is ready`,
          description: 'Restart to finish updating. The local engine stops first.',
          type: 'success',
          timeout: 0,
          actionProps: { children: 'Restart to update', onClick: () => void update.installAndRestart() },
        });
      } else if (s.phase === 'error') {
        show({ title: 'The update failed', description: s.message ?? 'Try again from Settings › Updates.', type: 'error', timeout: 8000 });
      } else if (shown.current) {
        toast.close(shown.current);
        shown.current = null;
      }
    };
    void update.state().then(onState);
    return update.onState(onState);
  }, [toast, update]);

  return null;
}
```

Register it in `web/plugins/desktop-settings/src/index.tsx` with `slots.register({ name: 'shell.overlay', plugin: 'desktop-settings' }, () => <UpdateToast update={desktop.update} />)` inside `ctx.effect`, and add `shell.overlay` to its slots. The Settings › Updates card stays. When the toast's description is "Download it now…", `download()` starts the self-update download, which is the consent (D1085).

- [ ] **Step 5: Notifications**

`apps/desktop-electron/src/preload/notification-shim.ts`, adapted from dsh-desktop's `src/preload/desktop-notification.ts`:

```ts
// Adapted from dataelement/dsh-desktop (MIT), src/preload/desktop-notification.ts.
// Page code that calls `new Notification(title, { body, data: { route } })` gets an OS
// notification through the existing shell bridge (`loamsDesktop.shell.notify`, D1085); a click
// focuses the window and opens `route` (main's onNotifyClick). Upstream injects a script and
// relays custom events; here the shim runs in the main world with contextBridge and calls the
// bridge the preload already exposes.
import { contextBridge } from "electron";

export function installNotificationShim(): void {
	contextBridge.executeInMainWorld({
		func: () => {
			const bridge = (globalThis as unknown as { loamsDesktop?: { shell: { notify(t: string, b?: string, r?: string): Promise<void> } } }).loamsDesktop;
			if (!bridge) return;
			class DesktopNotification extends EventTarget {
				static permission: NotificationPermission = "granted";
				static requestPermission = async (): Promise<NotificationPermission> => "granted";
				readonly title: string;
				readonly body: string;
				constructor(title: string, options?: NotificationOptions) {
					super();
					this.title = String(title);
					this.body = typeof options?.body === "string" ? options.body : "";
					const data = options?.data as { route?: unknown } | undefined;
					const route = typeof data?.route === "string" ? data.route : undefined;
					void bridge?.shell.notify(this.title, this.body, route);
				}
				close(): void {}
			}
			(globalThis as unknown as { Notification: unknown }).Notification = DesktopNotification;
		},
	});
}
```

In `preload/index.ts`, call `installNotificationShim()` after `contextBridge.exposeInMainWorld("loamsDesktop", api)`. Add its provenance row (`src/preload/notification-shim.ts ← src/preload/desktop-notification.ts`).

In `platform-web`, `notify` opens the route on click:

```ts
        async notify({ title, body, route }) {
          if (typeof Notification === 'undefined') return;
          if (Notification.permission === 'default') await Notification.requestPermission();
          if (Notification.permission !== 'granted') return;
          const n = new Notification(title, { body });
          if (route) {
            n.onclick = () => {
              globalThis.focus?.();
              globalThis.location.hash = route;
              n.close();
            };
          }
        },
```

In Studio's `notifications/data.ts`, `useNotifications(onArrive?)` keeps the time the page opened (`const opened = useRef(Date.now())`). For each new id with `createdAtMs > opened.current`, it calls `onArrive?.(n)` once. `NotificationsBell` takes `onArrive?: (n: StudioNotification) => void` and passes it to `useNotifications`. `header.tsx`'s `StudioContext` takes `notify?: (n: StudioNotification) => void` and renders `<NotificationsBell navigate={…} onArrive={notify} />`. In `index.tsx`, on the web only (`platform.kind === 'web'`; on the desktop Electron main posts these, W15), pass:

```tsx
            notify={(n) => {
              const kinds = [NotificationKind.APPROVAL, NotificationKind.QUESTION, NotificationKind.TURN_DONE, NotificationKind.ERROR, NotificationKind.GROUP_READY];
              if (!kinds.includes(n.kind) || document.hasFocus()) return;
              void platform.notify({ title: KIND_LABEL[n.kind], body: n.title, ...(n.href ? { route: n.href } : {}) });
            }}
```

Pass it through `registerStudioRegions`'s `StudioContext` entry. Add `notify?: (n: StudioNotification) => void` to `StudioRegionDeps`, and give it to `<StudioContext … notify={d.notify} />`.

- [ ] **Step 6: Run the tests**

```bash
pnpm --filter @loams/desktop test
pnpm --filter @loams/plugin-desktop-settings test
pnpm --filter @loams/plugin-studio test
pnpm -C web typecheck && pnpm --filter @loams/desktop typecheck
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop-electron web/packages/platform-web web/plugins/studio web/plugins/desktop-settings web/apps/console/src/cordis
git commit -s -m "feat(desktop): the update toast with consent and skip, and OS notifications on both editions (CF1 task 16)"
```

---

## CF1e — The quality gate (§7)

### Task 17: Screenshot tests, axe checks and the desktop chrome e2e

**Files:**
- Create: `web/apps/console/playwright.screens.config.ts`
- Create: `web/apps/console/test/screens/shell.screens.spec.ts`, `test/screens/tsconfig.json`
- Create (generated by the script, committed): `web/apps/console/test/screens/__screenshots__/*.png` (Q931)
- Modify: `web/apps/console/package.json` (`test:screens`, `test:screens:update`, `typecheck`; devDependency `@axe-core/playwright` 4.10.2)
- Modify: `web/plugins/studio/src/sidebar/Sidebar.tsx` (`data-elapsed` on `WorkingDuration`, so screenshots mask the ticking time)
- Modify: `.github/workflows/ci.yml` (the `web` job runs the screens and axe)
- Create: `apps/desktop-electron/test/e2e/chrome.spec.ts` (+ `test/e2e/chrome.spec.ts-snapshots/` baselines)
- Do not modify `scripts/ci/npm-licences.exceptions.json`: Task 0 found that `@axe-core/playwright` and `axe-core` (MPL-2.0) need no entry (T0-5)

**Interfaces:**
- Consumes: everything in CF1a–CF1d; `/?desktop` (the dev desktop preview with `fake-agentd.ts`; thread `demo-login`) and `/?demo` (W17).
- Produces: `pnpm --filter @loams/console test:screens` and `test:screens:update` (Q931: "updated by a script"); the axe gate; the desktop chrome check on CI's Linux runner.

- [ ] **Step 1: Write the screenshot and axe spec**

`web/apps/console/playwright.screens.config.ts`:

```ts
import { defineConfig, devices } from '@playwright/test';

// Design 59 §7: the shell in dark and light, against the dev server in its preview modes
// (W17: `?desktop` carries a fake agentd with threads; `?demo` is the web edition's mock).
// Baselines live in test/screens/__screenshots__ (Q931). Update them on Linux with
// `pnpm --filter @loams/console test:screens:update`; CI compares on ubuntu-24.04.
const port = Number(process.env.LOAMS_SCREENS_PORT ?? 5192);

export default defineConfig({
  testDir: 'test/screens',
  testMatch: '*.screens.spec.ts',
  timeout: 60_000,
  workers: 1,
  retries: 0,
  forbidOnly: !!process.env.CI,
  reporter: [['list']],
  outputDir: 'test-results/screens',
  snapshotPathTemplate: '{testDir}/__screenshots__/{arg}-{projectName}{ext}',
  expect: { toHaveScreenshot: { maxDiffPixelRatio: 0.01, animations: 'disabled', caret: 'hide' } },
  use: {
    ...devices['Desktop Chrome'],
    viewport: { width: 1280, height: 800 },
    deviceScaleFactor: 1,
    baseURL: `http://127.0.0.1:${port}/`,
    reducedMotion: 'reduce',
    timezoneId: 'UTC',
    locale: 'en-GB',
  },
  projects: [
    { name: 'dark', use: { colorScheme: 'dark' } },
    { name: 'light', use: { colorScheme: 'light' } },
  ],
  webServer: {
    command: `node node_modules/vite/bin/vite.js --host 127.0.0.1 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}/`,
    env: { LOAMS_UI_BASE: '/' },
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
```

The theme is System by default (Task 2), so each project's `colorScheme` picks dark or light.

`web/apps/console/test/screens/shell.screens.spec.ts`:

```ts
// The design 59 shell (§7): home, a thread with the inspector open, a classic page, the
// palette and the drawer, in dark and light (the two projects), each checked with axe.

import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, test } from '@playwright/test';

async function open(page: Page, query: '?desktop' | '?demo', route: string) {
  await page.goto(`/${query}#${route}`);
  await expect(page.locator('.lc-shell')).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
}

async function axe(page: Page) {
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map((v) => `${v.id} (${v.nodes.length}): ${v.nodes[0]?.target.join(' ')}`)).toEqual([]);
}

const mask = (page: Page) => [page.locator('[data-elapsed]'), page.locator('time')];

test('home', async ({ page }) => {
  await open(page, '?desktop', '/studio');
  await expect(page.getByRole('navigation', { name: 'Sidebar' })).toBeVisible();
  await expect(page.getByRole('list', { name: 'loams threads' })).toBeVisible();
  await expect(page).toHaveScreenshot('home.png', { mask: mask(page) });
  await axe(page);
});

test('a thread with the inspector open', async ({ page }) => {
  await open(page, '?desktop', '/studio/t/demo-login?panel=diff');
  await expect(page.getByRole('complementary', { name: 'Inspector' })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Diff' })).toHaveAttribute('aria-selected', 'true');
  await expect(page).toHaveScreenshot('thread-inspector.png', { mask: mask(page) });
  await axe(page);
});

test('a classic page', async ({ page }) => {
  await open(page, '?demo', '/data');
  await expect(page.getByRole('list', { name: 'Pages' })).toBeVisible();
  await expect(page).toHaveScreenshot('classic-page.png', { mask: mask(page) });
  await axe(page);
});

test('the palette', async ({ page }) => {
  await open(page, '?desktop', '/studio');
  await page.keyboard.press('Control+KeyK');
  await expect(page.getByRole('combobox')).toBeFocused();
  await expect(page).toHaveScreenshot('palette.png', { mask: mask(page) });
  await axe(page);
});

test('the drawer', async ({ page }) => {
  await open(page, '?desktop', '/studio/t/demo-login');
  await page.getByRole('button', { name: 'Show drawer' }).click();
  await expect(page.getByRole('region', { name: 'Drawer' })).toBeVisible();
  await expect(page).toHaveScreenshot('drawer.png', { mask: mask(page) });
  await axe(page);
});

test('every region and palette result is keyboard reachable (§7)', async ({ page }) => {
  await open(page, '?desktop', '/studio/t/demo-login?panel=diff');
  const reached = new Set<string>();
  for (let i = 0; i < 80; i++) {
    await page.keyboard.press('Tab');
    const where = await page.evaluate(() => {
      const el = document.activeElement;
      return el?.closest('.lc-side') ? 'sidebar' : el?.closest('.lc-topbar') ? 'topbar' : el?.closest('#lc-main') ? 'main' : el?.closest('.lc-inspector') ? 'inspector' : '';
    });
    if (where) reached.add(where);
  }
  expect([...reached].sort()).toEqual(['inspector', 'main', 'sidebar', 'topbar']);
  await page.keyboard.press('Control+KeyK');
  await page.keyboard.type('settings');
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('option', { selected: true })).toBeVisible();
});
```

`test/screens/tsconfig.json` is a copy of `test/e2e/tsconfig.json`, with `"include": [".", "../../playwright.screens.config.ts"]`. In `package.json`:

```json
    "typecheck": "tsc -p tsconfig.json && tsc -p test/e2e/tsconfig.json && tsc -p test/screens/tsconfig.json",
    "test:screens": "playwright test -c playwright.screens.config.ts",
    "test:screens:update": "playwright test -c playwright.screens.config.ts --update-snapshots"
```

Add `"@axe-core/playwright": "4.10.2"` to devDependencies and run `pnpm install`. In `Sidebar.tsx`'s `WorkingDuration`, add `data-elapsed=""` to the `<span>`.

- [ ] **Step 2: Run it and watch it fail**

```bash
pnpm --filter @loams/console exec playwright install chromium
pnpm --filter @loams/console test:screens
```

Expected: FAIL. Each `toHaveScreenshot` fails with `A snapshot doesn't exist at …/__screenshots__/home-dark.png, writing actual`, and the axe checks report any violation the earlier tasks left.

- [ ] **Step 3: Fix what axe finds, then write the baselines**

Fix each reported violation at its source with tokens and markup only, and record it as a `T17-*` ruling. Typical findings are a missing accessible name on an icon button or a contrast miss on a disabled state. Then, on Linux (the CI runner's OS, so fonts match):

```bash
pnpm --filter @loams/console test:screens:update
pnpm --filter @loams/console test:screens
```

Expected: the second run PASSES, with ten baselines written (five screens, two themes). Open each PNG and compare it with the mockups in §12 before committing. A baseline is the reviewed look.

- [ ] **Step 4: CI**

In `.github/workflows/ci.yml`, job `web` (working directory `web`), after `pnpm build`:

```yaml
      - name: Chromium for the screenshot tests
        run: pnpm --filter @loams/console exec playwright install --with-deps chromium
      - name: The shell in dark and light, with axe (design 59 §7)
        run: pnpm --filter @loams/console test:screens
      - uses: actions/upload-artifact@v7.0.1
        if: failure()
        with:
          name: console-screens
          path: web/apps/console/test-results/screens
          retention-days: 7
```

- [ ] **Step 5: The desktop chrome check**

`apps/desktop-electron/test/e2e/chrome.spec.ts`:

```ts
// Design 59 §7: the window chrome per OS on CI's Linux runner (D1083), plus a screenshot of
// the title bar area in both themes. No engine: the console's own boot screen is enough.
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { _electron as electron, expect, test } from "@playwright/test";
import { mainWindow } from "./windows";

const appRoot = resolve(__dirname, "../..");

test("the window chrome follows the OS and the theme", async () => {
	const work = mkdtempSync(join(tmpdir(), "loams-chrome-"));
	const userData = join(work, "user-data");
	mkdirSync(userData);
	const env: Record<string, string> = {};
	for (const [k, v] of Object.entries(process.env)) if (v !== undefined && k !== "ELECTRON_RUN_AS_NODE") env[k] = v;
	env.LOAMS_BIN = join(work, "no-engine");
	env.LOAMS_DESKTOP_USER_DATA = userData;
	const extra = (process.env.LOAMS_E2E_ELECTRON_ARGS ?? "").split(/\s+/).filter(Boolean);
	const app = await electron.launch({ args: [...extra, appRoot], cwd: appRoot, env });
	try {
		const page = await mainWindow(app);
		await page.waitForSelector(".lc-topbar");
		expect(await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().find((w) => w.webContents.getURL().startsWith("loams-app://console"))?.getMinimumSize())).toEqual([900, 640]);
		expect(await page.evaluate(() => document.documentElement.dataset.platform)).toBe(process.platform);
		if (process.platform !== "darwin") {
			// titleBarOverlay is on: the Window Controls Overlay API reports it.
			expect(await page.evaluate(() => (navigator as Navigator & { windowControlsOverlay?: { visible: boolean } }).windowControlsOverlay?.visible)).toBe(true);
		}
		const region = await page.locator(".lc-topbar").evaluate((el) => getComputedStyle(el).getPropertyValue("-webkit-app-region"));
		expect(region).toBe("drag");
		const button = await page.locator(".lc-topbar button").first().evaluate((el) => getComputedStyle(el).getPropertyValue("-webkit-app-region"));
		expect(button).toBe("no-drag");

		await page.getByRole("button", { name: /^Theme: / }).click();
		await page.getByRole("menuitem", { name: /Dark/ }).click();
		await expect(page.locator(".lc-topbar")).toHaveScreenshot(`titlebar-${process.platform}-dark.png`, { maxDiffPixelRatio: 0.02 });
		await page.getByRole("button", { name: /^Theme: / }).click();
		await page.getByRole("menuitem", { name: /Light/ }).click();
		await expect(page.locator(".lc-topbar")).toHaveScreenshot(`titlebar-${process.platform}-light.png`, { maxDiffPixelRatio: 0.02 });
		await expect
			.poll(() => JSON.parse(readFileSync(join(userData, "settings.json"), "utf8"))["ui.theme"])
			.toBe("light");
	} finally {
		await app.close().catch(() => undefined);
		rmSync(work, { recursive: true, force: true });
	}
});
```

The screenshot is of the page's top bar. Playwright cannot capture the OS-drawn overlay buttons, so the overlay's colours are checked by Task 13's unit test (Review Focus 3) and this spec's WCO check. Write the baselines on Linux:

```bash
pnpm --filter @loams/console build && pnpm --filter @loams/desktop build
LOAMS_E2E_ELECTRON_ARGS=--no-sandbox xvfb-run -a pnpm --filter @loams/desktop exec playwright test chrome --update-snapshots
LOAMS_E2E_ELECTRON_ARGS=--no-sandbox xvfb-run -a pnpm --filter @loams/desktop test:e2e chrome
```

Expected: the second run PASSES. `.github/workflows/desktop-electron.yml`'s Linux smoke step already runs `test:e2e`, so it picks up `chrome.spec.ts` with no workflow change.

- [ ] **Step 6: Run everything once**

```bash
pnpm -C web lint && pnpm -C web typecheck && pnpm -C web test
pnpm --filter @loams/console test:screens
pnpm --filter @loams/desktop test && pnpm --filter @loams/desktop typecheck
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add web/apps/console web/plugins/studio .github/workflows/ci.yml apps/desktop-electron/test pnpm-lock.yaml
git commit -s -m "test(console): design 59 screenshots in both themes, axe, and the desktop chrome check (CF1 task 17)"
```

The release notes for the PR that closes CF1 say: "A visual change: the console and Loams Desktop have a new look, layout and navigation (design 59). No plugin API changed; every page keeps its address." (D1086)

---

## Self-review

The plan was checked against design 59 on 2026-10-17. Its anchors (file lines, the slot table, `MIN_HEIGHT`, the vendored rows, the dsh-desktop headers) were read from `origin/dev` at `d8a810e3`, the merge of #706. Task 0 re-checks them when work starts.

### Spec coverage

| Spec item | Where | Tasks |
|---|---|---|
| D1073 tokens: names kept, T3 Code-derived values, dark B / light A, radius scale, composer shadow, softened pop and modal shadows, dialog glass, AA check, literals only in `tokens.css` | §3 | 1 (values, radii, shadows, glass tokens, `contrast.test.ts`, `colours.test.ts`); 3 (pages that looked wrong); 5 (dialog glass); W1–W5 |
| D1074 theme: OS-following, dark with no preference, per-device choice (local storage / desktop settings), `.dark` on `<html>`, native chrome synced | §3 | 2 (choice, storage, first paint, desktop setting); 13 (chrome sync) |
| D1075 type: system UI and mono stacks, mono only for code/ids/SQL, the scale, data cells 12–13px | §3 | 1 (stacks, scale utilities, Fontsource removed, NOTICE); 3 (`mono.test.ts`, the sweep); 4 (table cells) |
| D1076 density (28/32/36, top bar 44, rows 28, gutters 12–20) and motion off by default, optional easing ≤400ms, reduced motion wins | §3 | 1 (tokens, reduced-motion block); 2 (the motion setting); 4 (control heights); 5 (sheet and dialog transitions); 7 (top bar, gutters); 9, 10 (28px rows) |
| D1077 primitives: the rest of T3 Code's `components/ui` on Base UI, per-file provenance with verdicts, restyled on tokens, Loams-only primitives keep their API, no grain in the shell | §4 | 4 (controls, display, Empty without grain); 5 (overlays); 6 (toast, toastHelpers, sidebarState); W18 |
| D1078 six regions with slots; `inspector.open({ tab, props })`; slots via `@loams/slots` | §5 | 7 (sidebar, top bar, main, `shell.sidebar.*`, `shell.topbar.*`, `shell.home`); 8 (inspector, drawer, `shell.inspector.tab`, `shell.drawer.tab`, the services); 11 (`shell.palette.source`); W8, W9 |
| D1079 thread rows: status dot, elapsed time, pin, rename, drag-reorder, archive → Settings; status from Studio's store via agentd; projects | §5 | 9 (Studio's sidebar in `shell.sidebar.threads`, the cap, Archived in Settings); W6, W7 |
| D1080 navigation: pinned Pages per project (Q932 defaults), the palette for every page and command, Settings for org admin, section headings gone, routes kept | §5 | 10; 11 (palette sources); W20, W21 |
| D1081 keyboard table, Ctrl on Windows/Linux, one table in the shell | §5 | 12 (`SHELL_KEYMAP`, `useShellKeys`, Studio gives up the chords); 8 (interim ⌘I/⌘J); 14 (menu accelerators); W12 |
| D1082 toasts (vendored, stacked, bottom right, auto-dismiss, actions); destructive confirmations name object and impact; typed confirmation for production | §5 | 6 |
| D1083 window style per OS; top bar is the drag region with no-drag controls; `data-platform`/`data-fullscreen`; titleBarOverlay synced live; min 900×640; window state validated | §6 | 7 (drag CSS); 13; 17 (e2e) |
| D1084 menus (App, File, Edit, View, Window, Help; accelerators; Check for updates, Show logs, Restart local server, Report an issue); context menu; splash with the loader generator until ready; admin and desktop pages as full pages | §6 | 14; 15; 10 (Settings pages); W14 |
| D1085 OS notifications for approvals and finished runs when unfocused, click opens the thread, shim pattern on the preload bridge; update card → toast, consent before download, skip, restart, opt-out respected | §6 | 16; W15 |
| D1086 no flag; four steps that keep the console working; plugin pages fixed in the breaking step; release note | §8 | Execution order (CF1a–CF1d); 3; 17 (release note); W16 |
| §7 contrast, focus ring 2px offset 2px, keyboard reachability, screenshots (five screens × two themes, CI web job), axe, desktop e2e | §7 | 1 (contrast, forced colours); 4–5 (focus outline in the class table); 17 |
| §10 T3 Code and dsh-desktop provenance, LICENSE files, NOTICE, no `@deepseek-ai/*`, dependency rules | §10 | 4–6 (`web/packages/ui/provenance.json`); 15 (`apps/desktop-electron/provenance.json`, `LICENSE.dsh-desktop`, NOTICE, the no-deepseek test); 0 (dependency ages and licences) |
| Q928 no brand font · Q929 no grain · Q930 keymap later · Q931 baselines in the repo, updated by a script · Q932 default pins · Q933 vibrancy in light too · Q934 threads when agentd exists · Q935 mobile out of scope | §11 | 1 · 4 · 12 (comment) · 17 · 10 · 13 · 9 · none (tokens apply when the mobile app is restyled) |

### Placeholder scan

The plan was searched for "TBD", "TODO", "implement later", "similar to Task", "add appropriate", "handle edge cases" and an ellipsis inside a code block. The hits that remain are:
- `"<from the loop>"` and `"<sha256sum>"` in Task 15's `provenance.json` example. These are values the step's own command prints, and the step says which command fills each one.
- `--map apps/web/src/components/ui/<name>.tsx=src/<name>.tsx` in the shared vendoring recipe. Each task then lists its exact maps.

### Names and types

- `PanelService`, `PanelState` and `PanelTabProps` are defined once (Shared contracts, Task 8) and used with the same members in Tasks 9, 12 and 14.
- The tab ids are the same in Tasks 9, 12 and 17: `studio.diff`, `studio.files`, `studio.preview`, `studio.plan` and `studio.terminal`.
- `ThemeChoice` is declared in `@loams/console-host` (`theme.ts`) and mirrored as a string union in `apps/desktop-electron/src/shared/contracts.ts`, because the desktop main process does not import web packages. Task 2 tests both parse the same three values.
- The shell commands: `chat.new`, `thread.jump.N` and `thread.next` are Studio's command names in Tasks 7, 12 and 14. `palette.toggle`, `sidebar.toggle`, `inspector.toggle`, `drawer.toggle`, `navigation.back`, `navigation.forward`, `settings.open` and `thread.reopen` are the shell's (`ShellKeyCommand` and `ShellCommand`).
- `SIDEBAR`, `INSPECTOR` and `DRAWER` have the D1078 limits: 200–320 at 232, 240–640 at 280, and 160–560 at 280.

### Review Focus placement

| Review Focus | Test | Task |
|---|---|---|
| 1. Shortcuts in editable targets | `keymap.test.ts` "editable targets keep their keys" | 12 |
| 2. Many threads, long names | `shell-sidebar.test.tsx` "long names truncate and big projects collapse" | 9 |
| 3. Native chrome not following the theme | `window-chrome.test.ts` "chrome follows every theme change" | 13 |
| 4. Pages assuming the old frame | `viewport.test.ts` "no page sizes itself against the viewport" | 7 |
| 5. Forced colours | `contrast.test.ts` "forced colours (Review Focus 5)" | 1 |

## Owner answers (2026-10-17)

The owner accepted every recommendation ("yes accept all, run it that way"):

1. **W10, console and Studio modes:** yes. Part 2 retires the split: one shell, Studio's home at `/`, and Overview becomes a page. CF1 keeps both modes and the switcher.
2. **W1, light muted text:** accepted. `#6b665e` (muted) and `#716c64` (faint).
3. **W2, a second accent tone:** accepted. `--op-accent-fg` carries accent text; `--op-accent` is for fills and focus.
4. **W13, thread text in the palette:** for now, the title and the last message preview. Full transcript search comes with Part 2, through a new agentd method.
5. **Task 4, the T3 Code archive download:** approved. `scripts/studio/vendor.mjs fetch` downloads the GitHub archive zip by HTTP to record its hash; nothing is cloned.
6. **Task 0, `@axe-core/playwright` (axe-core is MPL-2.0):** allowed as a test-only dev dependency. It is never shipped, so record it in the licence gate as an exception for dev and test only.

**Execution method:** one implementer agent per task, in the order above. Each task opens its own PR into `dev`. CodeRabbit reviews it (self-review when CodeRabbit is rate-limited, said in the PR), and the PR merges only on a green `CI required` with `Connectors required` and `Web and mobile required` not failing. No reviewer subagents.

## Rulings made during execution

| # | Finding | Where | Absorbed by | Ruling |
|---|---|---|---|---|
| T0-1 | Anchors re-checked on `origin/dev` at `99c42834` (2026-10-17, the merge of #708). Nothing under `web/`, `apps/`, `scripts/`, `NOTICE` or the lockfile changed since `d8a810e3`, where the plan's anchors were read. `MIN_HEIGHT` is 600; `SLOT_KINDS` has 12 slots; `shell.dock.right` is registered only by `plugins/agent` (plus `plugins/shell/test/layout.test.tsx`); T3 Code is at `33aa4614…`, dsh-desktop at `bc732f5`; `NOTICE:40-45`, `web/README.md:86,90`, `MODE_HOME`, `shouldNotify`/`onNotice`, and the two viewport-sized pages (`plugins/factory/src/embedded.tsx`, `plugins/studio/src/layout/StudioLayout.tsx`) are where the plan says. Tech Stack versions match the catalog and `apps/desktop-electron/package.json`. Every T3 Code and dsh-desktop upstream path the plan cites exists in the local clones (T3 Code has no `ui/tabs.tsx`, which matches "Loams Tabs, not vendored"). | Step 1 | — | The plan holds as written, apart from T0-2 to T0-5. |
| T0-2 | Step 1's `text-accent\b` grep printed 31 on `.tsx` only, and with `.ts` it also counts `text-accent-ink` (`plugins/metrics/src/MetricsPage.tsx:325`), because `\b` matches before `-`. The test's own regex (`/\btext-accent(?![-\w])/`) over its file set finds **33** sites (30 in `.tsx`, 3 in `.ts`: `plugins/agent/src/markdown.ts`, `studio-ui/src/thread-list/logic.ts` ×2), plus **5** CSS rules `color: var(--op-accent)` (`packages/ui/src/styles.css:144,550,605`, `apps/console/src/cordis/console.css:122,377`). The plan said 34. | `web/` | 1 | W2 and Task 1 now say 33 sites and 5 CSS rules; the failing test in Task 1 Step 3 lists 38 hits. Step 7's `sed` already leaves `text-accent-ink` alone. Step 1's grep is corrected. |
| T0-3 | `grep -rln "Adapted from dataelement/dsh-desktop" apps/desktop-electron` prints 9 paths: the 8 adapted files and `apps/desktop-electron/NOTICE`, which quotes the header. The 8 files cite 7 distinct upstream paths (`src/main/runtime/harness-runtime.ts` backs both `engine/ports.ts` and `engine/supervisor.ts`); all 7 exist at `bc732f5`. | `apps/desktop-electron` | 15 | Step 1's grep excludes `NOTICE`. Task 15's `provenance.json` gets 8 "adapted before CF1" rows (W19), two of them with the same upstream path and hash. |
| T0-4 | Of the 12 rows in `web/packages/ui/provenance.json` (all at `6524a8d4`), 3 are unchanged at `33aa4614` (`kbd.tsx`, `spinner.tsx`, `toggle-group.tsx`) and 9 changed: `collapsible.tsx`, `CommandPalette.logic.ts` (→ `command-rank.ts`), `command.tsx`, `input-group.tsx`, `middle-truncate.tsx`, `scroll-area.tsx`, `sidebar.tsx`, `skeleton.tsx`, `switch.tsx`. The local T3 Code clone is shallow (one commit), so the size of each change cannot be diffed locally. | `web/packages/ui` | 4–6 | As the plan says: not re-vendored; the existing rows stay at `6524a8d4` (W18). An implementer who touches one of the 9 files reads the `33aa4614` file before restyling it, and does not change its provenance row. |
| T0-5 | `@axe-core/playwright@4.10.2`: MPL-2.0, published 2025-06-03, no dependencies besides `axe-core ~4.10.3` (resolves to 4.10.3, MPL-2.0, published 2025-03-05) and the peer `playwright-core >= 1.0.0` (the catalog has 1.63.0). `gifenc@1.0.3`: MIT, published 2021-03-07, no dependencies. All are well past the 14-day rule. MPL-2.0 is on `deny.toml`'s allow list, and `scripts/ci/npm-licences.mjs` checks only production dependencies (`pnpm licenses list --prod`), so a devDependency is not scanned at all. dsh-desktop's `scripts/generate-loader-gifs.mjs` does `require('sharp')`, which confirms W14. | `scripts/ci`, Tasks 15, 17 | 17 | No exception entry: the gate allows MPL-2.0 and never sees a devDependency, so an entry would be dead data. This meets owner answer 6 (axe is allowed as a test-only dev dependency) without changing the gate. Task 17 leaves `npm-licences.exceptions.json` alone, and its file list now says so. |
| T1-1 | `ThreadStatusLabel`'s two dots are `size-[7px] rounded-pill` and `size-1.5 rounded-pill`, not the `size-[9px] rounded-full` and `h-1.5 w-1.5 rounded-full` Step 6.8 names. | `studio-ui/src/thread-list/StatusIndicators.tsx` | 1 | `data-status-dot=""` went on both spans as intended. |
| T1-2 | `studio-ui/test/thread-list.test.ts` "uses token classes only" pinned the pill's text class to `text-(accent\|info\|danger\|grow)`, so the W2 move broke it. It was not in Task 1's file list. | `studio-ui/test` | 1 | The pattern now allows `text-accent-fg` instead of `text-accent`. |
| T1-3 | Every contrast pair passed with the plan's values on the first run; no token value changed. The tailwind test's first failure was `--ease-panel` (checked before `--radius-none`) plus the unmapped `--op-scrim`, which the widened `colorTokens` regex now finds. | `web/packages/ui/src/tokens.css` | — | The values in Step 4 are the values as built. |
| T1-4 | Only `color:` declarations moved to `--op-accent-fg`. `text-decoration-color: var(--op-accent)` (`admin-ui/src/admin.css`) and `--xy-minimap-mask-stroke-color` (`plugins/canvas/src/canvas.css`) are UI marks, not text, and keep the accent (3:1 is met). `web/packages/ui/README.md`'s "Dark mode" paragraph (humus and chalk) was rewritten for directions A and B along with "Fonts". | `web/` | 1 | As built. |
| T1-5 | Biome reflowed three lines that the `text-accent-fg` rename made too long (`ui/src/overlay.tsx`, `plugins/git/src/CodeBrowser.tsx`, `StatusIndicators.tsx`). `overlay.tsx` gained 2 lines inside `SelectMenu`, and `console.css` lost 2 lines at the top. | Tasks 5, 7 | 5, 7 | Task 5's `SelectMenu`/`Combobox` anchor is now `overlay.tsx:304-422` (Tabs stays `423-473`); Task 7's `/* Frame */` anchor is now `console.css:38-598`. |
| T1-6 | Review of #710: two `text-accent` sites in `studio-ui/src/git/WorktreeSetupCard.tsx` (a warning stage, and "worktree ready, setup action failed") used the old ochre to mean "warning". Indigo means no state. CodeRabbit also asked for a 3:1 control-border token for `Question.tsx`'s answer fields, which use `border-rule` (about 1.2:1). | `studio-ui`, Task 4 | 1, 4 | Both WorktreeSetupCard sites use `text-warn` (W3), not `text-accent-fg`. No new token for control borders in Task 1: W4's `--op-input` is the input outline, and Task 4 restyles the fields on it. Task 4 decides whether `--op-input` needs a 3:1 value for non-text contrast (WCAG 1.4.11). |
| T2-1 | `web/plugins/shell/test/palette.test.tsx` "palette_scopes_by_mode" pinned the old single `Use dark theme` row under `# General`, twice. It was not in Task 2's file list. | `plugins/shell/test` | 2 | Both expected lists now carry `# Appearance` with `Theme: System`, `Theme: Dark` and `Theme: Light`. |
| T2-2 | A platform that calls `ctx.provide('theme', …)` must list it in its manifest's `provides`. | `packages/platform-web`, `packages/platform-electron` | 2 | Both `package.json` manifests gain `"theme"` in `provides`. |
| T2-3 | Small changes to the plan's code. `useThemeState` subscribes through one stable callback (a `.bind` per render made `useSyncExternalStore` resubscribe on every render). `AppearanceMenu` keeps the motion flag in state, so its item label changes after a toggle. `desktopThemeStore` swallows a rejected `theme:set`, so the choice still holds for the window. The shell also exports `useThemeState`. | `plugins/shell/src/appearance.tsx`, `packages/platform-electron` | 2 | As built. |
| T2-4 | `layout.tsx` lost 20 lines (`useTheme`, `THEME_KEY` and the sun/moon button). | Task 7 | 7 | Task 7's anchors are now `layout.tsx:713-717` (the context or environment switcher), `706-806` (the `lc-shell` tree) and `693-704` (the engine check, the gate and the bare-page branch). |
| T2-5 | Settings › Appearance renders wherever `console.settings.section` is shown, which today is only the desktop's Settings page (`plugins/desktop-settings`). On the web, the header's Appearance menu and the palette's three `Theme:` commands carry the choice. Chromium reports `prefers-color-scheme: light` when the OS states no preference (Playwright's `no-preference` included), so the dark fallback is reachable only where `matchMedia` gives no answer (jsdom, or no `matchMedia`). | Task 10 | 10 | Task 10's Settings place should render the `console.settings.section` entries (Appearance among them) on the web too. Screens and tests that need dark set it with `colorScheme: 'dark'` or a saved choice, never by relying on "no preference". |
| T3-1 | The scan found **236** lines in **95** files, not about 120 in about 60. Of them, 192 got a `mono-ok:` marker (106 ids, 52 code, 23 paths, 8 SQL, 3 hashes), 19 became `tabular-nums` (counts, durations, sizes, diff stats, axis ticks), and 24 lost mono as labels (14 uppercase headings became `text-micro font-medium text-faint`, in sentence case; GroupGrid's "winner" chip is now "Winner"). One hit, `studio-ui/src/diff/diffRendering.ts:326`, is vendored `as-is`: `provenance.test.ts` pins its hash, so it cannot carry a marker. | `web/packages/ui/test/mono.test.ts` | 3 | The test skips files whose row in `packages/studio-ui/provenance.json` or `packages/ui/provenance.json` has the verdict `as-is`. The plan's `export function monoHits` lost its `export` (Biome's `noExportsInTest`). Markers in JSX use the plan's form `className={/* mono-ok: id */ '…'}`; in CSS (and CSS in template strings) `/* mono-ok: … */`; in plain TS a trailing `// mono-ok: …`. Review of #713: the `<code>`/`<pre>`/`<kbd>`/`<samp>` exception now holds only when the mono face is on that element's own opening tag, so an unmarked label beside a `<code>` on the same line is caught. |
| T3-2 | Without mono, `.loams-status`, `.loams-badge`, `.loams-table th` and `.loams-stat > span` at 0.66–0.7rem fell below the type scale's 11px floor. None of the four had letter-spacing or uppercase to drop. | `web/packages/ui/src/styles.css` | 3, 4 | All four take `font-family: inherit` and `font-size: var(--op-text-micro)`. `.loams-table th` goes from weight 400 to 500 so the faint header still reads. `.loams-snippet` keeps mono. Task 4 restyles these primitives from these values. |
| T3-3 | `.lc-preview` (the "Preview" chip in the header and the hosted screens) was mono, uppercase and 0.6rem. | `apps/console/src/cordis/console.css` | 3 | It is sans, sentence case ("Preview"), `--op-text-micro`, weight 500. |
| T3-4 | Metrics: the selected dashboard tab was `bg-accent-soft text-accent-ink`. Task 1 made `--op-accent-ink` white (ink *on* an accent fill), so in light the selected tab's text vanished. Both tabs were also unreset `<button>`s, so the unselected ones showed the browser's grey button face in dark. | `plugins/metrics/src/MetricsPage.tsx` | 3 | Selected: `border-0 bg-accent-soft text-accent-fg`. Unselected: `border-0 bg-transparent text-muted`, `hover:bg-raised`. `text-accent-ink` is for text on `bg-accent` only (it stays so in `console.css`'s `::selection` and the org initial). |
| T3-5 | SQL: the database list showed list bullets and the grey browser button face, and its hover used `--loams-surface-2`, which does not exist. | `plugins/sql/src/SqlPage.tsx` | 3 | The `ul` is `m-0 list-none p-0`. Each row is `rounded-sm border-0 bg-transparent text-ink hover:bg-raised`, and the current one is `aria-[current=true]:bg-accent-soft`. |
| T3-6 | Step 4's walk: `/ui/?demo` serves only Status, Namespaces, Deployments, Approvals and the hello page. Every other console route in `?demo` says "Nothing here". `/ui/?desktop` serves Canvas, Connectors (and instances), Durable, Git, Jobs, Live (and deployments), Metrics, Pipelines, Postgres, Settings, SQL, Streams, WeSQL and Studio (home, run group, threads, factory). Analytics and Graph show their "could not reach the server" state. Data, Members, Teams and Audit are served by neither mode. Every page was screenshotted in dark and light (`colorScheme`), and computed fonts were checked: the only mono headings left are file paths in Studio's diff matrix. Connectors' card heads and Jobs' panels still look square next to rounded controls. | Tasks 4, 5, 17 | 4, 5, 17 | Only Metrics (T3-4) and SQL (T3-5) failed the "looks wrong" test. The square panels are Task 4/5's restyle. Task 17's screenshot list should reach the console pages through `?desktop`, because `?demo` does not serve them. |
| T4-1 | T1-6 left open whether `--op-input` needs 3:1. The Global Constraints require 3:1 for UI boundaries, and the field outline is how a field is found (WCAG 1.4.11); `#dcd7cf` and white at 8% were about 1.4:1 and 1.2:1. | `web/packages/ui/src/tokens.css`, `contrast.test.ts` | 4 | `--op-input` is 3:1 on every surface: light `#8a857c` (3.67 on white, 3.28 on the window, 3.19 on raised), dark `rgb(255 255 255 / 0.36)` (3.26 or more over every backdrop). `contrast.test.ts` "control outlines (--op-input) are 3:1" checks it. Fields carry it (InputControl, TextareaControl, NumberFieldGroup, SelectTrigger, CheckboxControl, Radio, `.loams-input`), and `studio-ui` `Question.tsx`'s answer fields move from `border-rule` to `border-input` (the #710 review). A button is named by its label, so `.loams-btn-secondary` and `.loams-btn-danger` take half of it (`color-mix(in oklab, var(--op-input) 50%, transparent)`) instead of Step 4's full `--op-input`, which drew a heavy outline round every button. |
| T4-2 | Base UI parts type `className` as a string or a state function, and `cx` joins strings only, so `tsc` refused every vendored part that passes `className` to `cx`. | `web/packages/ui/src/cx.ts` | 4, 5, 6 | `cx.ts` exports `WithClass<P>` (`Omit<P, 'className'> & { className?: string }`), the shape `collapsible.tsx` already used; the vendored parts take `WithClass<…Props>`. A state-function `className` is not accepted on `@loams/ui` parts. Tasks 5 and 6 use the same type. |
| T4-3 | Two of Step 1's tests fail in jsdom for test reasons: Base UI checks the next radio only after the focus move settles, and a Select item is not picked by a bare `click` event. `mono.test.ts` named the `.loams-badge {` rule, which Step 4 deletes. | `web/packages/ui/test` | 4 | The radio test awaits `act` after the arrow key; the select test sends `pointerDown`, `pointerUp`, `mouseUp` and `click`, as `studio-ui/test/git.test.tsx` does. `mono.test.ts` now asserts that the `.loams-badge` rule is gone and that `badge.tsx` has no mono face. |
| T4-4 | Small changes to the plan's code. `Alert` keeps upstream's child sorting (icon, then title and description, then `AlertAction`) and `controlAlignment`; the glass and sidebar surfaces are dropped. `select.tsx` keeps `SelectButton`. Upstream's popups sit at `z-[130]`; the console's sheet is `z-[71]` and the toaster `z-[80]`, so picker popups sit at `z-[75]` (above dialogs, below toasts). The shared picker classes and glyphs are module exports, not package exports: `POPUP_SURFACE`, `POPUP_ITEM`, `GROUP_LABEL` and `ChevronDown` in `select.tsx`; `POPUP_LAYER`, `INPUT_BUTTON`, `ChevronsUpDown` and `Cross` in `combobox.tsx`; `CONTROL_BOX` in `input.tsx`. | `web/packages/ui/src` | 5 | Task 5 rebuilds `SelectMenu`/`Combobox` and the overlays on these, and keeps menus, popovers and tooltips between the dialog and the toaster layers. `SelectMenu` and `Combobox` stay in `overlay.tsx` until Task 5 (the File structure line now says so). |
| T4-5 | `.loams-btn` rules are unlayered, so `ButtonGroup`'s Tailwind utilities (in the `utilities` layer) cannot square a Button's corners. | `web/packages/ui/src/styles.css` | 4 | `styles.css` joins `.loams-btn` children of `[data-slot="group"]` itself (corners and the shared border, both orientations). Upstream's hover and focus tricks on the group separator are dropped. |
| T4-6 | The import writes `localSha256` for the upstream bytes plus the header; every existing `adapt` row holds the hash of the file as adapted. `command.tsx`'s comment at lines 6–9 also said autocomplete was folded in. | `web/packages/ui/provenance.json`, `src/command.tsx` | 4 | Each new row's `localSha256` is the adapted file's hash. `command.tsx` keeps commit `6524a8d4`; its provenance note (Step 5) and its comment now point at `src/autocomplete.tsx`, so its `localSha256` was refreshed too. The archive row for `33aa4614` is `cd024c09…77518`. |
| T5-1 | The plan put the dialog backdrop and viewport at `z-50`. The console's mobile sidebar (`.lc-side`) is at 60 and Studio's narrow sidebar at 50, so a `z-50` dialog would open under them. | `web/packages/ui/src/dialog-styles.ts`, `popover.tsx`, `tooltip.tsx` | 5, 6 | Dialogs and sheets keep the old layers: scrim 70, viewport 71. Menus, popovers and preview cards sit at `z-[75]` (`OVERLAY_LAYER` in `popover.tsx`, the same layer as the pickers), tooltips at `z-[76]`, and the toaster stays at `z-[80]`. The `.loams-dialog-scrim` rule stays in `styles.css`, because the command palette's backdrop uses it. |
| T5-2 | `cx` joins classes and does not merge Tailwind utilities, so two utilities for the same property (a corner, a text size, `flex` against `grid`) both land and the stylesheet order decides. | `web/packages/ui/src/{popover,menu,sheet}.tsx` | 5, 6 | Each part picks one value instead of overriding a shared one: `OVERLAY_SURFACE` carries no corner or text size (each part adds `rounded-panel text-body`, or `rounded-sm text-small` for the tooltip-style popover), menu rows share a base without a display or gap, and the Loams `Sheet` renders its 44px title on the primitive title rather than restyling `SheetTitle`. Task 6 should follow the same rule. |
| T5-3 | Seven `SelectMenu` callers pass `className` as the whole trigger look (the project and environment switchers, Studio's composer pills, `EnvModeSelector`); `SelectTrigger`'s own classes would have stacked under theirs. | `web/packages/ui/src/select.tsx` | 5 | `SelectTrigger` gains an additive `unstyled` prop, and `SelectMenu` sets it when `className` is given. Without `className`, the trigger is `SelectTrigger` at size `lg` (36px, the field height). The default `icon` is the chevron glyph instead of `▾` (the prop is unchanged), and the picked row is tinted (`data-selected`) instead of carrying a ✓. |
| T5-4 | The Loams `Combobox` never had an open button. In jsdom, Base UI opens the combobox list on a key, not on a synthetic `change` event. | `web/packages/ui/src/combobox.tsx`, `test/primitives-overlays.test.tsx` | 5 | The wrapper renders `ComboboxInput` with `showTrigger={false}`. The test opens the list with ArrowDown, types, then picks with the pointer sequence (T4-3). |
| T5-5 | Upstream parts that need T3 Code's app: the popover `panel` variant (thread-details width, `--z-sheet`), the menu item `ghost` variant (`buttonVariants`), the tooltip `glass` variant (`dropdown-glass`), the `DropdownMenu*`/`*Content`/`*Overlay` aliases, and dialog `variant="media"`. | `web/packages/ui/src` | — | Dropped, each named in its file's header. The tooltip `code` variant stays, with a `mono-ok: code` marker. Beyond the plan's list the package also exports `DIALOG_VIEWPORT_CLASS`, `DIALOG_MOBILE_SHEET_CLASS`, `Dialog/AlertDialog/SheetBackdrop`, `…Portal`, `…Viewport`, `SheetSide`, `MenuItemLabel`, `MenuRadioItemIndicator`, `PopoverCreateHandle` and `TooltipScrollDismissArea`. `Sheet`'s `side` gains `top` (a widening). |
| T5-6 | The plan's `.loams-dialog-body` dropped `display: grid; gap: 16px`, which the existing dialogs' forms rely on for the space between fields; and a tall dialog had no scroll. The plan also switched the `AlertDialog` danger button from `danger` to `danger-solid`. | `web/packages/ui/src/styles.css`, `alert-dialog.tsx` | 5 | The body keeps its grid and gap and scrolls (`min-height: 0; overflow-y: auto`), and `.loams-dialog` (the Loams `Dialog`'s popup) is capped at `100dvh - 32px`, so the head and foot stay. The danger confirm is `danger-solid`, as the plan says. Screens checked in dark and light: the theme, account and environment menus, Studio's thread menu, composer pickers and delete confirmation, and the Streams, Postgres and Connectors dialogs. |
| T5-7 | The import wrote each row's `localSha256` for the upstream bytes plus the header. `Toaster`, `ToastProvider` and `useToast` moved to `src/toast.tsx` without a header or row. | `web/packages/ui/provenance.json`, `src/toast.tsx` | 6 | As T4-6: the 8 rows hold the adapted files' hashes. Task 6's `vendor.mjs import` overwrites `toast.tsx` (the tool does not refuse an existing file), so Task 6 must keep `Toaster`, `ToastProvider` and `useToast` exported with the same API. |
| T6-1 | Base UI removes a closed toast after its ending transition, which it schedules from an effect after the close renders. `vi.advanceTimersByTime(1500)` inside one `act` fires the 1000ms timer but never the removal, so "auto-dismisses after its timeout" kept the title. | `web/packages/ui/test/toast-confirm.test.tsx` | 6 | The test advances with `await vi.advanceTimersByTimeAsync(1500)`, which lets the effect run between timers. Step 1's code says so. |
| T6-2 | The plan's `Toaster` was a plain `flex-col-reverse` column, which drops the stack that `buildVisibleToastLayout` exists for. | `web/packages/ui/src/toast.tsx` | 6, 16 | `Toaster` keeps upstream's stack at the bottom right only: the newest toast in front, older ones peeking 12px behind it at 90%, 80%… scale, spread out while the pointer or focus is on the stack (Base UI's `data-expanded`); swipe right or down dismisses. The transforms run on `--op-panel-duration` (0 unless Motion is on, D1076), opacity and height on `--op-duration`. The dismiss button is upstream's corner button (it calls `data.onClose`), hidden on the toasts behind the front one until the stack spreads. Width is `min(360px, 100vw - 32px)`, the surface `rounded-lg border-rule bg-surface shadow-pop`. `Toast.Title` is an `<h2>` and `Toast.Description` a `<p>`, and the console has no preflight, so both carry `m-0 text-body`. `ToastData` also keeps `actionLayout`, `actionVariant`, `secondaryActionVariant` (as Loams `ButtonVariant`s) and `expandableLabels`, which `stackedToast` needs; `additionalActions`, `expandableDescriptionTrigger`, the anchored toasts and the copy-error button are dropped (the row's note names them). `type: 'danger'` is normalised by `useToast` and by the `Toaster` itself, so `toastManager.add({ type: 'danger' })` from outside React (Task 16) also shows as an error. `useToast()` returns a memoised `ToastApi` (`add`, `close`, `update`), stable across renders, so Task 16's `[toast]` effect dependency does not re-run on every toast. `ToastProvider` always uses `toastManager` (a `toastManager` prop is ignored). |
| T6-3 | `sidebar-state.ts` is vendored `as-is` in double quotes; `biome check --write` rewrote it to single quotes, which breaks the as-is hash. | `web/biome.json` | 6 | A Biome override keeps double quotes for `packages/ui/src/sidebar-state.ts`, as for `studio-ui`. |
| T6-4 | `InputControl` and `TextareaControl` (Task 4) left the native `<input>`/`<textarea>` with the browser's border, face, size and colour, because the console has no preflight: the typed confirmation showed a second border inside the outline, and `font="mono"` never reached the text. No page used them yet outside `@loams/ui`. | `web/packages/ui/src/{input,textarea}.tsx` | 6 | The inner control is `border-0 font-[inherit] text-inherit` and takes the box's text size (`text-[length:inherit]`, or `text-small` at `sm`/`compact`, one value per T5-2). Both provenance rows' `localSha256` were refreshed. Toasts (every type, an action and a secondary action, collapsed and spread) and the typed confirmation (empty and filled) were checked in dark and light. |
| T7-1 | `admin.css`'s sign-in `min-height: 100svh` is at line 512, not 516. The hosted edition's manifest (`apps/console/src/hosted/edition.tsx:79`) also lists `shell.header.server`, and `plugins/desktop-servers/package.json` declares it. | Task 7 | 7 | The marker is on line 512. Both manifests now declare `shell.sidebar.server`. |
| T7-2 | Four of Step 1's test lines fail for test reasons. The test's session returned a new principal object per call, so `UserMenu`'s `useSyncExternalStore` looped ("Maximum update depth"). jsdom's `URL` is not Node's, so `readFileSync(new URL(…))` threw. Biome wraps the long `.lc-topbar :is(…)` selector onto two lines, which the regex's single space missed. Base UI's scroll area (now in the sidebar) calls `Element.getAnimations`, which jsdom lacks; this also broke `apps/console/test/modes.test.ts`. A second `separator` (the sidebar's) made `layout.test.tsx`'s unnamed `getByRole('separator')` ambiguous. | `plugins/shell/test`, `apps/console/test/modes.test.ts` | 7 | The test keeps one principal object, resolves the stylesheet with `fileURLToPath`, and matches `\.lc-topbar\s+:is(`. `plugins/shell` gains `test/setup.ts` (a `setupFiles` entry: `getAnimations` and `ResizeObserver` stubs) and `@types/node` 24.13.6 as a devDependency (already in the lockfile for other packages). `modes.test.ts` stubs `getAnimations`. The dock test names its separator ("Resize agent panel"). `layout.test.tsx`'s "Open navigation" drawer test is gone; `frame.test.tsx` adds "toggles the sidebar with Mod+B, but not while typing", "is a sheet below 960px, opened from the top bar" and "opens the palette from the top bar". |
| T7-3 | Small changes to the plan's code. `HeaderContext` is not moved: `TopBar`'s `Entries` renders `shell.header.server` the same way. `Brand`'s close button is optional and the frame passes none (the sheet has its own close button, and the top bar toggles the column). `@loams/ui`'s `Sidebar` takes no `id`, so the column sits in a `display: contents` `<div id="lc-side">` for the toggle's `aria-controls`. `nav.ts` also carries `ORG_SECTIONS`, `NavItem`, `LEGACY`, `isActive`, `useLocation` and `useModeFilter`, which the frame and the sidebar share. On the home route `MainRegion` sets the window title to "Loams" (the outlet that names it is not mounted there). | `plugins/shell/src` | 8–10 | As built. `index.tsx` exports `SIDEBAR`, `readSidebar`, `useSidebarState`, `useNarrow`, `runShellCommand`, `TopBar`, `MainRegion`, `ShellSidebar` and the nav model from `nav.ts`. |
| T7-4 | The plan's `Layout` dropped the Agent dock toggle from the bar, but the dock stays until Task 8 and its tests must keep passing. | `plugins/shell/src/layout.tsx` | 8 | The dock toggle is `TopBar`'s `trailing`: a 28px icon button (`.lc-bar-btn`, `aria-label="Agent"`), without the old "Agent" text. Task 8 passes `PanelToggles` there instead; its toggles now use `.lc-bar-btn`, which styles `aria-pressed="true"` as on. |
| T7-5 | D1078 makes the sidebar collapsible with ⌘B, but the keymap comes only in Task 12, and a plain listener would take bold from the rich-text composer (Review Focus 1). | `plugins/shell/src/layout.tsx` | 12 | A temporary Mod+B listener toggles the sidebar unless the target is a field, `contenteditable` or an `.xterm` terminal (`keepsKeys`). Task 12 deletes it with Task 8's listener; its text says so. |
| T7-6 | Step 5 replaces console.css lines 40–590 wholesale, but that range also holds rules pages use (`.lc-page`, `.lc-page-head`, `.lc-muted`, `.lc-cards`, `.lc-actions`, `.lc-icon-btn`, `.lc-chip`, `.lc-round-btn`, `.lc-bare`, `.loams-boot`). The plan's `.lc-main` had no gutter, so every Console page would touch the card's edge. A page's absolutely placed `sr-only` label (SQL) had no positioned ancestor inside the scroller, so it stretched the document by 155px. | `apps/console/src/cordis/console.css` | 8 | Those rules stay, on the type scale and the radius steps. `.lc-main` is `position: relative` with a 24/20/56px gutter (20/12/48 under 960px), and its children keep the 1180px measure (Studio's are `max-width: none`; a Console page in Studio keeps the 20px gutter). The rise-in animation on `.lc-main > *` is gone (motion off, D1076). Under 960px the card is edge to edge whether or not the sheet is open. In the sidebar the desktop server switcher is full width and its menu stays inside the column; the hosted edition's engine row wraps. The splitter between the sidebar and the card is invisible until hovered or focused. Checked through `?desktop` (Connectors, SQL, Deployments, Studio) and `?demo` (Status, Deployments) in dark and light, at 1440px and 800px: the top bar is 44px, the sidebar 232px and dragging it stops at 200 and 320, Ctrl+B collapses it, Ctrl+K opens the palette, below 960px it is a sheet, no page scrolls the document or sideways, and Studio fills the card with one scroller per column. |
| T8-1 | Step 1's "hides tabs whose routes do not match" finds the `Diff` tab while it is the only tab, but Step 5 drew the tab strip only for two or more tabs. The plan's focus effect also ran on the first paint (a saved open inspector took focus on load) and never gave focus back on close, which the old dock did. | `plugins/shell/src/regions.tsx` | 9 | The strip shows with one tab too; it names what the panel holds. Focus moves only on a change: into `[data-panel-autofocus]` (or the old `[data-dock-autofocus]`, which the agent's composer carries) or the region on open, and back to the toggle (`data-panel-toggle="inspector"`/`"drawer"`) when it closes with focus inside it or dropped to the body. The toggles carry `aria-controls` (`lc-inspector`, `lc-drawer`), and the splitter `controls`. Tab props default to one frozen empty object. `regions.test.tsx` adds "Mod+I and Mod+J toggle the panels, but rich text and a terminal keep them", "focuses the tab's marked field on open, and the toggle when it closes from inside" and "shows an old shell.dock.right entry as an inspector tab". |
| T8-2 | Step 5's temporary Mod+I/Mod+J listener had no editable-target check, so ⌘I in the rich-text composer would toggle the inspector instead of italic, and Ctrl+J in a terminal would not reach the shell program (Review Focus 1). | `plugins/shell/src/layout.tsx` | 12 | It skips `contenteditable` and `.xterm` targets (`keepsPanelKeys`). A plain field still toggles, as the old dock's ⌘J did, so the agent's composer can close its own panel. Task 12 deletes the listener and the helper (its Step text says so). |
| T8-3 | The plan's `.lc-inspector`/`.lc-drawer` drew a `border-left`/`border-top` next to `ResizeHandle`'s own 1px line, which doubled the hairline; and nothing stopped a wide inspector or tall drawer from squeezing main to nothing. | `apps/console/src/cordis/console.css` | — | The regions have no border: the splitter (`.lc-panel-handle`, `--op-rule`, the accent on hover or focus) is the hairline. The inspector is at most `100% - 240px` of the card and the drawer `100% - 120px`. The tab strip sits in `.lc-panel-tabs` (8px side padding); the narrow sheet's body is `.lc-inspector-sheet`. The `.lc-dock*` rules are gone. Checked through `?desktop` in dark and light at 1440 and 800 with two temporary drawer tabs (not committed; no plugin registers a drawer tab until Task 9): the inspector opens at 280 with the Agent tab and focus in its composer, drags to 640 and stops, the drawer opens at 280 under main and the inspector, both open together, below 960px the inspector is a sheet from the right, and no page scrolls the document. |
| T8-4 | The agent plugin's test pinned its manifest's slots to `shell.dock.right`. | `plugins/agent/test/panel.test.tsx` | 8 | It expects `shell.inspector.tab`. `shell.dock.right` stays declared and in the shell manifest; the inspector shows an entry in it as a tab with the id `dock` (no plugin registers one now). |
| T9-1 | Loading every available mode at boot (W10) broke two of `console-host/test/mode.test.ts`'s pins: "boots the stored mode" listed the applied plugins without Console's, and `other_mode_plugins_load_on_first_entry` expected Studio to stay deferred in Console. A catalog row narrows a manifest's `inject`, so the `studio` rows in `catalog/base.yml` and `catalog/desktop.yml` also needed `inspector` and `drawer`, which `apps/console/test/modes.test.ts` pins. | `packages/console-host/test`, `apps/console/{catalog,test}` | — | The stored-mode test lists `pages` too. The lazy-loading test is now `other_mode_plugins_load_at_boot_after_the_boot_mode`: Studio is active in Console without a switch or a navigation, and later switches apply nothing again. A mode that is not available (the web `studio` gate) still defers its plugins ("a mode flag hides the mode" covers it). Both catalog rows and the catalog test carry `inspector, drawer`. |
| T9-2 | The thread list renders only once an agentd is reachable (Q934), and it is no longer inside `StudioView`, so every Studio test that found the sidebar, the panel or the terminal next to the view failed. | `plugins/studio/test` | — | A test harness, `test/shell.tsx`, renders `StudioInShell`: `StudioThreads`, the view, and stand-ins for the inspector (`complementary` "Inspector" with Studio's four tabs) and the drawer (`region` "Drawer") on a `TestPanel` (the shell's `PanelController` without storage). `sidebar.test.tsx`'s `show()` awaits the Projects navigation. Its archive scenario now opens Studio settings, finds the thread in the "Archived threads" list and unarchives it. `studio.test.tsx`'s three layout tests (sizes, toggles, narrow overlays) are one test of the new `StudioLayout`: no splitter or panel of its own, and `rightPanel.toggle`/`show` and `terminal.toggle` reach the inspector and the drawer. `desktop.test.tsx` passes `navigate` to `SettingsPage`. |
| T9-3 | Small changes to the plan's code. A router whose `current()` returns a new object (the plan's own test stub) loops `useSyncExternalStore`, so `regions.tsx` reads the route through a string snapshot. On Studio's home the page outlet is not mounted, so Mod+N, the palette's Studio commands and `?host=` would have stopped working there. The deep-link effect's extra dependencies failed Biome's `useExhaustiveDependencies`. Without the old wrapper's gutter, the Diff and Files bodies touched the inspector's edges. | `plugins/studio/src/{regions,view}.tsx`, `settings/Archived.tsx` | 10–12 | `StudioHome` takes `keys`, `navigate` and `inspector` and runs `useStudioPageKeys` (exported from `view.tsx`: the route's commands and keybindings, which `StudioPage` uses too); it also selects `?host=`. The deep link keys its effect on `studioHref(thread)`, so a new file link reopens Files. `show(panel)` also calls `inspector.open` itself (the link may already name the panel). `Tools` passes `openTerminal` only on a thread route (the terminal tab is `THREAD_ROUTES` only). `StudioView` drops its `factory` and `gitBus` props (the sidebar and the top bar own them now) and the `Title` component. `ThreadPanelBody` keeps a `px-3 pt-2` body. `ArchivedThreads` is a `Card` (not an `Empty`), shows nothing while no agentd is reachable, and names its button "Unarchive <title>". `StudioRegionDeps` gains an optional `mode` (T9-4); Task 16's `notify` addition still applies. |
| T9-4 | The desktop e2e `factory_group_visible_in_studio_only` pins the sidebar's Factory group (links to `/studio/factory`) to Studio mode, but W10 shows Studio's thread list in Console too, with the Factory group inside it. | `plugins/studio/src/regions.tsx` | — | The threads show in both modes; the Factory group only in Studio mode, as its nav entry does (`useStudioMode` over the `mode` service). `factory.test.tsx` "shows in Studio mode only; the threads show in both" covers it. |
| T9-5 | `StudioSidebar` brought its own 44px "Projects" header with a border, its own scroller and a bordered foot into the shell's sidebar, which already scrolls. Without an agentd the slot entry renders nothing, but the shell's "Threads" section kept its padding. | `plugins/studio/src/sidebar/Sidebar.tsx`, `apps/console/src/cordis/console.css` | 10 | The header is a 28px label row (Projects, Add), the search sits under it, the list does not scroll by itself, and the foot ("Clean up worktrees", "Studio settings") is two 28px rows without a border. Status and error lines are `text-small`. `.lc-side-threads:empty` hides the section, so the sidebar has Pages only (Q934). Task 10 decides whether the foot's "Studio settings" link stays once Settings is a place of its own. |
| T9-6 | The old Studio panel was open by default at 420px on every thread; the shell's inspector starts closed at 280px, so a thread's diff shows only after ⌘I, "Show inspector" or a `?panel=` link. The shell's tab strip switches tabs with `inspector.open` and does not write `?panel=`. At 280px the five tabs (Studio's four and the agent's) overflow the strip, and the Diff tab's own toolbar wraps. | `apps/console/test/e2e`, `apps/desktop-electron/test/e2e`, `console.css` | 17 | As built: the inspector keeps the shell's default. The e2e helpers gain `inspector(page)` and `openInspectorTab(page, tab)`; `diffOf` reads the "Inspector" complementary; `thread_runs_and_shows_diff` and the approval scenario open the Diff tab; `files_tab_reads_the_checkout` no longer expects `panel=files` in the hash; `terminal_round_trip` presses "Show drawer". The same changes went into the desktop spec. `.lc-panel-tabs` scrolls sideways (`overflow-x: auto`, thin scrollbar). The web e2e passed locally (14 of 14, with the 2026-10-15 debug `loams-agentd`); the desktop Studio e2e needs xvfb and was not run locally. A wider default for Studio's tabs is left to the owner (Task 17's screenshots will show it). |
| T9-7 | Studio's `sidebar.toggle` command was handled by `StudioLayout` on a narrow window only (its overlay sidebar). | `plugins/studio/src/layout/StudioLayout.tsx` | 12 | It has no Studio handler now; the shell's Mod+B (Task 7's listener, Task 12's keymap) toggles the one sidebar. Screens checked through `/?desktop` in dark and light at 1440 and 800: the home slot with the threads in the sidebar, Console mode (Connectors) with the threads above the Pages, a thread with the Diff and Files tabs, the drawer's Terminal tab, Studio settings with Archived threads, and the narrow inspector sheet; no page scrolls the document. |
| T9-8 | CI's web job timed out "long names truncate and big projects collapse" (`plugins/studio/test/shell-sidebar.test.tsx`) at 5380ms against vitest's 5s default: it renders 400 threads and expands 250 (Review Focus 2), which takes about 0.6s locally but far longer while CI runs 47 projects' tests at once. Merging dev brought #732 (owner feedback below), which removed the web `studio` gate that T9-1 cites as the deferred-mode example. | `plugins/studio/test/shell-sidebar.test.tsx` | — | The test keeps its scale and gets a 20s budget, as `apps/console`'s boot tests do; `threadLinks` reads the list with a selector instead of `getAllByRole`. T9-1's deferral still holds for an edition that sets `BootOptions.modeFlags`. #732's behaviour is kept: the web Studio empty state, the switch in every edition and the 1600px centred page measure. Checked through `/ui/?desktop#/studio/t/demo-login` at 1920px in dark and light: one sidebar (the shell's, with the threads, the Factory group and the Pages), the thread loads, and the document does not scroll. |
| Owner feedback 2026-10-17 | In the hosted preview, pages kept T7-6's 1180px measure, left-aligned, so a 1920px window left about 460px empty on the right; and the Console \| Studio switch was missing on the web because Studio stayed behind the `studio` instance feature (W10, SD1's Task 32 gate). | `apps/console/src/cordis/{console.css,start.ts}`, `plugins/streams/src/pages`, `plugins/studio/src/home/states.tsx` | — | Amends T7-6 and W10. A page's measure is 1600px, centred (`.lc-main > :not(.lc-home)`), so tables and grids fill the card at 1440 and 1920; `.lc-page`, `.ds-page` and the streams pages drop their own 1180px caps; prose keeps `.lc-page-head p`'s 68ch; Studio keeps its own full-width layout. `startConsole` no longer gates Studio on an instance feature, so the switch shows in every edition; a browser with no agentd host shows "Studio needs a Loams server with agentd" with a link to Studio settings › Agentd hosts. `BootOptions.modeFlags` stays for an edition that wants a gate. |
| T10-1 | The plan's "Edit pages" menu used `MenuItem` with a hidden `Check` icon, so a screen reader heard no pinned state and the menu closed after each pick. | `plugins/shell/src/sidebar.tsx`, `test/pages.test.tsx` | — | The rows are `MenuCheckboxItem`s (`menuitemcheckbox`, `aria-checked`), and the menu stays open while pins are toggled. Settings pages are not offered (Settings lists them). An empty list says "No pinned pages. Pin one here or from the palette." `pages.test.tsx` finds `menuitemcheckbox`, and adds "marks the current page, the most specific one" and "pins a page from the palette, and the sidebar follows". |
| T10-2 | The plan's palette added a pin or unpin row for every page, so an empty query listed every page twice. | `plugins/shell/src/{layout,palette}.tsx` | 11 | `ShellPalette`'s `nav` takes the query (`nav(mode, query)`). "Go to" rows are under "Pages" or "Settings" in every mode, as the current mode's (no "Go to" rows under the other mode's heading); the pin rows come only once something is typed, under "Sidebar", `secondary`. `studio-home` is not listed, so the old "Go to Home" row is gone (the brand and New thread go there). `palette.test.tsx` "palette_scopes_by_mode" now expects `# Pages` with a Studio page in Console. |
| T10-3 | T2-5: Appearance had no Settings place on the web, where no plugin serves `/settings`. Registering the shell's own `/settings` would win over the desktop's (the router takes the first match, and the shell loads first). | `plugins/shell/src/{preferences,pages,index}.tsx` | 12 | The shell serves "Preferences" at `/preferences` (group `Settings`): every `console.settings.section` entry, one under the other. `pageEntries` leaves it out when a page with the id `settings` exists (the desktop's page lists the same sections), and `settingsHome` tries `settings`, `teams`, `org-settings`, `preferences`, then `studio-settings`. So on the web without org pages, the foot's Settings and ⌘, open Preferences. |
| T10-4 | Three Settings pages were all labelled "Settings" (the desktop's, the org's, and Studio's), which the Settings list shows side by side. | `plugins/{desktop-settings,org,studio}/src/index.tsx` | — | Their page labels are "App settings", "Organisation settings" and "Studio settings". Routes, page titles and headings do not change. |
| T10-5 | T9-5 left open whether the Studio sidebar foot's "Studio settings" link stays once Settings is its own place. | `plugins/studio/src/sidebar/Sidebar.tsx` | — | It is gone: the shell foot's Settings and the Settings list reach Studio settings, and Studio's own palette command (Ctrl+,) stays. The foot keeps "Clean up worktrees". |
| T10-6 | Owner feedback 2026-10-17: the sidebar's Factory rows used a larger type (`0.88rem`) than the thread rows, and its eight apps pushed the Pages far down at 1920px. | `plugins/studio/src/sidebar/Factory.tsx`, `test/factory.test.tsx` | 17 | The rows are the thread row's box and type (28px, `text-small`, `rounded-md`, `row-hover`), and status labels are `text-micro`. The group is collapsed by default: a 28px disclosure row ("Factory", then "n of 8 connected" with a danger dot when one fails) and the "All factory apps" link, which stays visible, so the desktop e2e `factory_group_visible_in_studio_only` still finds `a[href="#/studio/factory"]`. The choice is per device (`loams.studio.factory.open`). A list failure shows whether the group is open or not. Checked at 1920 in both themes: with Factory collapsed the Pages sit just under the threads; open, the eight rows match the thread rows. |
| T10-7 | `apps/console`'s cordis, modes and hosted tests read the old grouped nav (`.lc-nav a`) to learn which pages the plugins registered. With pinned Pages the sidebar shows only a few. | `apps/console/test/{cordis,modes,hosted}.test.*`, `plugins/shell/test/{layout,mode,console-pages}.test.tsx` | — | They read the page model from `handle.slots` through `pageEntries` (exported by `@loams/plugin-shell`); the OSS catalog's list gains the shell's "Preferences", and the cordis boot test also checks the sidebar pins `['Deployments']` (the only Q932 default that catalog serves). After switching to Studio the modes test waits for "Studio settings". `layout.test.tsx`'s section-grouping test is deleted; "existing_routes_still_render" checks where legacy pages land (Status, Namespaces and Odd are Pages; Approvals is Settings, current in the Settings list). `mode.test.tsx` checks that the pinned Pages are the same in both modes. `console-pages.test.tsx` checks that `shell.topbar.left` shows in the top bar and `shell.nav.context` is no longer rendered. |
| T10-8 | Small changes to the plan's code. Pages are mode-agnostic (D1080), so a Console page pinned in Studio switches the mode when opened (`syncModeWithRoute`). `currentProject` falls back to the last project opened, while the project pill falls back to the first project, so on a fresh device "Agents" (a `:project` page) is not in the Pages until a project is opened once. | `plugins/shell/src/{pages,sidebar,frame}.tsx`, `plugins/projects/src` | 11 | `pages.ts` also exports `pageAt` (the most specific page at a path, shared by the sidebar and the crumbs), `togglePinned`, `PAGES_CHANGED_EVENT` and `PREFERENCES_ID`; with storage blocked, pins hold in memory for the page load. `Crumbs` are the root (the instance, or Studio in Studio mode), the page the route sits under when it is not the page itself, then the page title. The branch pill (W21, `EnvironmentSwitcher`, now in `frame.tsx`) and the project pill (`ProjectPill`, `shell.topbar.left`, Console only) are 28px chips (`.lc-pill-trigger`, like Studio's context chips) and hide below 720px. The `.lc-nav*`, `.lc-switcher*` and `.la-side*` rules are gone. The Settings list keeps the way back while moving between Settings pages. Screens at 1920 and 1440 in both themes: Console home, a Studio thread, the palette (empty and "pin"), Settings and Preferences; the document never scrolls. At 1440 in Studio the top bar's palette hint is clipped by Studio's context chips; this task did not change that bar, so Task 17's screens should look at it. |
| T10-9 | Review of #736 (CodeRabbit): T10-8 hid both pills below 720px, which left no way to switch project or environment on a narrow window; on the web the org's home ("All projects", group Organisation) is at `/`, so opening the console put the sidebar in Settings; and the collapsed Factory header's danger dot was the only sign of a failing app, hidden from assistive technology. | `console.css`, `plugins/shell/src/sidebar.tsx`, `plugins/studio/src/sidebar/Factory.tsx` | — | Amends T10-8. Below 720px the pills stay, capped at 120px. The route `/` always shows Pages (it stays a Settings row in the palette and the Settings list). The collapsed header adds visually hidden text, "Some apps need attention.", beside the decorative dot. `pages.test.tsx` "the home route stays in Pages where it is the org home" and `factory.test.tsx` cover the last two. |
| T9-6a | Owner answer to T9-6 (2026-10-18): a Studio thread should show its diff without a keypress, as the old Studio panel did. | `plugins/shell/src/{panel,layout}.tsx`, `test/regions.test.tsx` | 17 | Amends T9-6. `PanelConfig` gains route `profiles`; `INSPECTOR` has one, `THREAD_INSPECTOR`: on `/studio/t/:threadId` the inspector opens on the Diff tab (`studio.diff`) at 420px. A profile keeps its own saved state per device (`loams.shell.inspector.thread`), so a person's resize, tab or close on a thread holds for threads, and every other route keeps the closed 280px default and its own saved state (`loams.shell.inspector`). `Layout` applies the profile in a layout effect (`PanelController.route`), so a thread never paints without its panel. Below 960px the inspector is a sheet over the thread, so there the profile's default is closed (the saved choice still applies). The profile lives in the shell next to `MODE_HOME`, not in Studio, because `PanelService` is the host's interface and a new method on it would be a breaking change of the host. `regions.test.tsx` "a Studio thread opens the inspector on Diff at 420px, and keeps the person`s choice there" covers it; the e2e helper `openInspectorTab` already accepts an open inspector. |
| T11-1 | The Search work (#769, open) rewrites `plugins/data-studio/test/data.test.tsx` and edits `src/index.tsx`, both in this task's file list. | `plugins/data-studio/{src/index.tsx,test}` | — | `TablesSource`'s tests are in a new `test/palette.test.tsx`. `data.test.tsx` changes one assertion only: "data_studio_renamed_label_only" counts the `shell.nav.section` entries, since the plugin now also registers a palette source. `index.tsx` gains the registration and one `SqlPage` prop, away from #769's hunks. |
| T11-2 | Owner brief 2026-10-18: a table in the palette should open the SQL editor (the shared editor, #762) or Search, not only the collection. | `plugins/data-studio/src/{palette.tsx,pages/sql.tsx,index.tsx}` | — | Each table has its row (the collection, as planned) and two `secondary` rows, "Search <table>" (`/data/:ns/:table/search`) and "Query <table> in SQL" (`/data/:ns/sql?table=<table>`), all under "Tables" with the namespace as detail. The router joins a query to `params`, so `SqlPage` takes `table` and opens on `SELECT * FROM "<table>" LIMIT 100` (the namespace's default schema is `collections`; quotes are doubled), and a new `?table=` while the page is open replaces the text. The row says "SQL", as the page does: the Data explorer's SQL is DataFusion over collections, and "Loams SQL" names the MySQL-compatible database page. |
| T11-3 | Contributed rows come before the "Go to" rows, so an empty query would have listed every project, table and branch above the Pages, and a few repositories with many branches would fill the palette. | `plugins/{projects,data-studio}/src/palette.tsx`, `plugins/studio/src/palette/BranchesSource.tsx` | 17 | As T10-2's pin rows, the three new sources publish only once something is typed. The tables load when the palette opens, so typing finds them at once; branches are read only once typed. The plan's `ProjectsSource` test now types "docs", and each test also checks that an empty query publishes nothing. Threads keep their recent list on an empty query. |
| T11-4 | `TablesSource` needs the open environment's namespace; the desktop has no environment until the engine reports one, and the web catalog row narrows data-studio's `inject` (T9-1). | `plugins/data-studio/{src/index.tsx,package.json}`, `apps/console/catalog/web.yml` | — | The namespace is `session.environment()?.namespace`, else `default` (as `DataRoute` does), and nothing is listed while the server serves no `loams.collection.v1`. `session` joins the manifest's `inject` and the web row's. |
| T11-5 | Small changes to the plan's code and tests. `stubRouter` records navigations in `navigated` and does not move `current()`. A test agentd whose `reach()` returns a new object loops `useSyncExternalStore` (as T9-3). `if (!namespace) return publish([])` returned the publish result from an effect, which React then called as a destructor. | `plugins/{projects,data-studio,studio}`, `packages/studio-ui/provenance.json` | — | The projects test checks `router.navigated`; the branches test's fake keeps one `reach` object, serves `ListBranches` from a Connect router, and checks that no `SwitchRef` is sent and that nothing is read on an empty query; the effect returns nothing. `PaletteThread.preview` is the last keyword, after the id, so a pasted id still outranks a message. `palette/logic.ts`'s provenance row has the adapted file's new `localSha256`. |
| T11-6 | `/ui/?desktop` serves no data plane and no admin API, so the palette had no tables or projects to show there. | screenshots only | 17 | The screens (1440, dark and light: empty Console and Studio, projects, tables, pages, commands, branches, threads, thread text) ran against `?desktop` with a Playwright route mock that answers `GetInstance`, `WhoAmI`, `ListCollections` and `ListProjects`; the mock is not committed. The palette's search field shows two focus rings (the input's and its group's), as in T10-8's screens; Task 17 should look at it. |
| T12-1 | The plan's `matchShellKey` compared `e.key` only. The listeners it replaces also took `e.code` (`KeyK`), so on a Cyrillic or Greek layout, or AZERTY's digit row, every shortcut would have stopped working. | `plugins/shell/src/keys.ts` | — | A press stands for the layout's character and, where the layout types no Latin letter or digit there, the physical key (`KeyK`, `Digit7`), as T3 Code resolves it. A Latin layout's own letter wins, so Dvorak's ⌘K is the key marked K. `keymap.test.ts` "reads the physical key when the layout types another character". |
| T12-2 | Step 5 filtered Studio's terminal pass-through to `metaKey` chords on Linux and Windows. That dropped every shell chord there, including Ctrl+Shift+P, Ctrl+Tab, Ctrl+, and Ctrl+Shift+M, which the keymap still takes from a terminal; the old list passed Ctrl+K and Ctrl+J into the console. | `packages/studio-ui/src/keybindings/settings.ts`, `plugins/studio/src/thread/TerminalDrawer.tsx` | — | `SHELL_KEYBINDINGS` rows (`ShellKeybinding`) carry `terminal: true` for the chords the shell takes from a Linux or Windows terminal. `isStudioShortcut` passes those there and every chord on macOS. `keymap.test.ts` checks the flag against each `SHELL_KEYMAP` row's `when`, so the two tables cannot drift; `terminal.test.tsx` covers both platforms. |
| T12-3 | With Mod+N and Mod+1…9 gone from `DEFAULT_KEYBINDINGS`, the sidebar's jump hints (held Ctrl/⌘) found no binding and disappeared, and Studio's palette labelled "New thread" Ctrl+Shift+O. | `packages/studio-ui/src/keybindings/{shared,store}.ts`, `plugins/studio/src/{sidebar/Sidebar,palette/StudioPalette}.tsx` | — | `SHELL_RUN_KEYBINDINGS` lists the shell's chords for Studio's commands (Mod+N `chat.new`, Ctrl+Tab `thread.next`, Mod+1…9 `thread.jump.N` on the desktop only, as before). `KeybindingStore.shown()` puts them between the defaults and the custom rules, so a custom rule is still the one shown. The palette and the hints read `shown()`; key presses still resolve against `resolved()`, which lacks them. |
| T12-4 | T7-5's listener left Mod+B to any field; D1081's table (Step 3) keeps it only in rich text and a terminal. Studio's composer is `contenteditable` in plain-text mode too. | `plugins/shell/src/keyboard.tsx`, `test/frame.test.tsx` | — | As planned: in a plain field Mod+B toggles the sidebar (a field has no bold), as T8-2 let Mod+I and Mod+J through. `contextOf` reads Studio's `data-composer-rich-text`, so the plain-text composer gives Mod+B and Mod+I to the shell, and any other `contenteditable` counts as rich text. `.xterm` counts as a terminal as well as `[data-terminal]`. `frame.test.tsx`'s Mod+B test now checks that ⌘ does nothing on Linux and that rich text keeps the key. |
| T12-5 | Step 4's Esc ran `closeOverlay` whatever the target, so Esc in a menu over the narrow sidebar sheet could close both, and it took the key even when nothing closed. | `plugins/shell/src/keyboard.tsx` | — | Esc from inside a `menu` or `listbox` is left to it; `overlay.close` calls `preventDefault()` only when it closed the narrow sidebar or inspector sheet. A key another handler took (`defaultPrevented`) or an IME composition is ignored. `keyboard.test.tsx` covers each. |
| T12-6 | Studio's keybinding editor lists only commands that have a binding, so `settings.open`, `sidebar.toggle`, `navigation.back`/`forward` and the jumps have no row any more, and Studio's palette shows no shortcut for "Studio settings" (⌘, opens the Settings home, T10-3). `palette.test.tsx`'s editor scenario rebound `settings.open`. | `plugins/studio/test/palette.test.tsx` | Q930 | As built: those chords are the shell's, and a user-editable shell keymap is Q930. The scenario rebinds `rightPanel.toggle` (Mod+Alt+B) and checks that Ctrl+B is flagged as "Shell: Toggle Sidebar". `sidebar.test.tsx`'s keyboard test sends `thread.jump.2` and `chat.new` as `SHELL_COMMAND_EVENT`, as the shell does. |
| T12-7 | The file list has `wireShellCommands` forward "the new menu commands, Task 14", but those `ShellCommand` names arrive with Task 14 (#820, open, which adds `menu-commands.ts` on `SHELL_COMMAND_EVENT`). Step 5 had `listenShellCommands` run any Studio command it is sent, and Studio has its own `settings.open` (Studio settings) and `sidebar.toggle`, so the menu's Settings could open Studio settings, depending on which listener was added first. | `plugins/studio/src/palette/commands.ts`, `packages/platform-electron` (unchanged) | 14 | Task 12 leaves `wireShellCommands` alone. `listenShellCommands` runs only Studio's half (`chat.new`, `thread.next`, `thread.previous`, `thread.jump.N`); `settings.open`, `sidebar.toggle` and `navigation.*` on the event are the shell's (`desktop.test.tsx` "the shell's commands"). `useShellKeys` has one `run(command)` switch; whichever of #820 and this PR merges second folds `runMenuCommand` into it, so a chord and a menu click share one path, and the menu's ⌘⇧T uses the keymap's stack. |
| T12-8 | Mod+N, Mod+Shift+T and Mod+1…9 are the browser's in the web edition, and Chrome and Firefox do not deliver them to the page. | `plugins/shell/src/keys.ts` | — | The keymap binds them in every edition. On the web the browser keeps them, and the palette, the sidebar's New thread and the thread rows still reach the same commands. The jump hints show on the desktop only (T12-3). |
| T-UX-1 | Walkthrough (#783) item 5: the hosted header's engine dot was green whatever the engine did, even after "Connect anyway" to one that was down. | `apps/console/src/hosted/{engine.ts,edition.tsx}`, `console.css`, `test/hosted.test.tsx` | — | `watchHealth` polls `<engine>/health` with `checkHealth` (no credentials): every 30 s while it answers; after a failure 2 s, doubling to at most 60 s; at once on `focus` or `online` (not while the tab is hidden). The dot is amber while the first check runs (`--op-warn`), green when it answers, red (`--op-danger`) when not. The tooltip and visually hidden text say which, and when down the reason ("No answer. Check that it is running and that this device is on its tailnet.") and the next try. A retry after a failure keeps the dot red, so it does not flicker amber. `HostedHeader` takes a `watch` prop for tests. |
| T-UX-2 | Walkthrough item 2: the Pages list showed only Deployments, Git and Connectors on the desktop, so Overview and every data page were reachable only through the palette. | `plugins/shell/src/{pages.ts,sidebar.tsx,all-pages.tsx,icons.ts,index.tsx}`, `plugins/data-studio/src/{index.tsx,pages/collections.tsx}`, `console.css` | 10 | Amends T10's `DEFAULT_PINNED` (Q932): `data`, `data-sql`, `search`, `streams`, `graph`, `live`, `analytics`, then the old `deployments`, `git`, `agents`, `connectors`; a page the edition does not serve is skipped as before, and stored pins are never rewritten, so a person who changed their pins keeps them. The Data explorer registers two more Pages: "SQL" (`/data/default/sql`, the namespace `/data` opens) and "Search" (`/search`, new: the namespace's collections, each opening its Search tab; `?ns=` picks the namespace). The Pages list ends in "All pages" (`/pages`, the shell's, no nav entry of its own): every non-Settings page under Data, Compute, Integrate, Studio, Factory and More, each with a pin toggle (`aria-pressed`, "Pin Jobs" / "Unpin Git"). With threads above, the row can sit below the fold at 900px; the sidebar scrolls. `pages.test.tsx` covers the page, the stored-pins rule and the new defaults; the mode and cordis tests read `.lc-pages ul a` (the pinned rows only). |
| T-UX-3 | Walkthrough item 3: on the desktop the sidebar's Settings list showed "App settings" beside the foot's "Settings", the page's heading was "Settings" again, and Preferences repeated Appearance and Servers. | `plugins/shell/src/{pages.ts,sidebar.tsx,preferences.tsx,index.tsx}`, `plugins/desktop-settings/src/{index.tsx,settings-page.tsx}`, `console.css` | 10 | Amends T10-3 and T10-4. The foot's Settings stays the one entry. The Settings list keeps one heading ("Settings") and groups its pages under App (`settings`, `preferences`), Organisation (the org's pages) and Studio (`studio-settings` and group `Studio`), an empty group left out (`settingsSections`). The desktop's page and tab title are "App settings" (it holds Appearance, Servers, Agent providers, Local stacks, Updates, About and Background agents). `/preferences` replaces itself with `/settings` where the desktop's page exists, so it is not a second place; on the web it is still the settings page. Labels in the list stay as T10-4 set them, since the palette shows them without the group. |
| T-UX-4 | Walkthrough item 4: Graph, Analytics, Streams, Live, Studio home and the Data explorer said "Loading…" or "Asking the server…" in text; `Skeleton` was unused. | `packages/ui/src/{skeleton-list.tsx,index.ts}`, the plugins' pages | — | `@loams/ui` gains `SkeletonList` (a new file, so the T3-derived `skeleton.tsx` and its provenance row do not change): one `role="status"` labelled with what loads, over `Skeleton` bars (`rows`, 28px, varied widths) or blocks (`cards`, 96px grid). Cards where a page is still asking what the server serves (Graph, Analytics) and on Studio home; rows for lists and tables (Graph's list and schema, Streams & Links' list, link and stream, Live's tables, documents and deployments, the Data explorer's collections and documents). The Studio sidebar's and Factory's small "Loading…" rows stay text: they sit inside a 28px row. |
| T-UX-5 | Walkthrough item 7: Overview's Activity and Git showed a "Sample data" badge and the shared banner. | `plugins/{overview,git}/src`, their tests | — | The banner (`SampleDataNotice`) stays and the badge goes; the tests count one "Sample data". `SampleDataBadge` stays in @loams/ui for a place with no banner. |
| T-UX-6 | Walkthrough item 6: the Storefront canvas fitted all ten services at 1440, about 0.45 zoom, node text near 6px. | `plugins/canvas/src/CanvasPage.tsx` | — | `FIT_VIEW = { padding: 0.15, minZoom: 0.75, maxZoom: 1 }` for the first fit and Auto-layout: a project wider than the canvas opens readable around its middle, with the minimap and zoom controls for the rest, and a small one no longer opens at 2x. The canvas' own zoom range (0.25 to 2) and the Fit view control are unchanged. |
| T-UX-7 | Walkthrough item 8: the desktop's `/data` said "No plugin serves /data" over loams-apps-mock. The desktop catalog row was right; the manifest's `requires.api: [loams.collection.v1]` gated the whole plugin (D938), and the mock's GetInstance does not list that API (it serves the REST collection reads only), so no Data explorer route existed, and its default pin was skipped. | `plugins/data-studio/{package.json,src/index.tsx,test/data.test.tsx}` | — | The manifest asks for no API; the plugin still injects `flags` (re-applied when they change). `DataRoute` already answers a server without the API with "This server has no data plane.", now worded for both editions, and the palette's tables publish nothing there (T11-4). So the page explains instead of the shell's "Nothing here", on any server. Against the mock the desktop shows that notice; the mock serving `loams.collection.v1` over Connect would be a Rust change, out of scope here. |
| UI1-1 | Owner brief 2026-10-19 (UI upgrade, phase 1): Motion for React as the one animation library; hover about 120–180ms, pages 180–280ms, springs only for interactive elements, transform and opacity only, reduced motion always wins. D1076 had motion off by default. | `packages/ui/src/{tokens.css,motion.tsx}`, the overlay primitives | — | Amends D1076. Panels (sidebar, inspector, drawer) still snap unless Settings › Appearance › Motion is on (`--op-panel-duration`). Overlays (menus, popovers, selects, tooltips, dialogs, the palette) open in 160ms on `--op-ease-out` from their trigger and close in 110ms on `--op-ease-in`, through Base UI's `data-starting-style`/`data-ending-style` (CSS, so Base UI's unmount, focus return and dismissal are unchanged). Toasts rise in 220ms (they snapped, on the panel duration). `Tabs` and `Segmented` slide their selection on a spring (`SelectionIndicator`, a Motion shared layout). `motion@13.5.1` (MIT, 2026-10-02, the last 13.x; 14.0.0 is the same day's new major) is in the catalog; the npm licence gate passes. `prefers-reduced-motion: reduce` zeroes every `--op-duration-*`, and every Motion preset becomes an instant change (`REDUCED_PRESET`). |
| UI1-2 | Phase 1 inspection: cards, stats, notices, snippets and meters were square (the CF1 radius scale reached controls and overlays only); the card head was a raised band at 0.92rem/540; notices carried a 3px side stripe; status dots were squares; about 20 sizes sat off the type scale (`0.72–0.95rem`, weights 520/540/560); Tabs had no focus ring; Button had no pressed or loading state; there was no icon-only button with a required name, no search field and no general list row; menus and popovers used `--op-surface`, the same step as the cards under them. | `packages/ui/src/*` | — | Cards and stats take `--op-radius-lg`, notices and snippets `-md`, the meter a pill; the card head is a hairline, no fill, `--op-text-emphasis` 600. Notices are a tone fill with a tone dot beside the title (never colour alone: the title says what happened). Every size is on the scale; the palette's and sidebar's uppercase headings are sentence case `text-micro` 500 (as T3-1). New tokens: `--op-elevated` (floating layers; dark `#171717`), `--op-accent-hover`/`-active`, `--op-danger-hover`, `--op-control-border`, `--op-focus`/`-width`/`-offset`, `--op-control-xs`, `--op-row-comfortable`, `--op-icon-*`, `--op-weight-*`, `--op-shadow-toast`, and the motion durations and curves; `contrast.test.ts` checks the hover fills and the elevated surface. New exports: `IconButton`, `SearchInput`, `ListRow`, and the motion module. |
| UI1-3 | T11-6's two focus rings on the palette's search field, and on every `InputControl`, came from `console.css`'s unlayered `:focus-visible` rule, which beats the layered Tailwind `outline-none` on the inner input, whatever its specificity. | `apps/console/src/{tailwind.css,cordis/console.css}` | — | The default ring moves into a `base` cascade layer, ordered below the utilities (`@layer theme, base, utilities`), and uses `--op-focus`. A control that draws its own ring (a field's box, a menu row's highlight) now opts out with `outline-none`. The palette, the sidebar search and every field show one ring. `tsup`'s declaration build needs `LIST_STAGGER` typed as `Variants` (TS4023). |
| UI2-shell | Owner brief 2026-10-19 (UI upgrade, phase 2: shell and navigation), and CodeRabbit on #801. | `plugins/shell/src/{sidebar.tsx,frame.tsx,regions.tsx,panel-motion.ts}`, `packages/ui/src/{command.tsx,list-row.tsx,input.tsx,styles.css}`, `console.css`, `plugins/shell/test/navigation.test.tsx` | — | Keeps UI1-1's rule: the sidebar, inspector and drawer snap unless Settings › Appearance › Motion is on. With it on, a toggle slides the panel's size on `--op-ease-panel` over `--op-panel-duration` (`usePanelToggle` reads the token at the toggle, so reduced motion and Motion off stay instant, and a drag or a keyboard resize stays instant); the panel holds its size inside a clip anchored to its outer edge, so its content does not reflow, and a closing panel is `inert`. The current page's chip is a `SelectionIndicator` that slides between the sidebar's rows. The Pages list and Settings' groups (App, Organisation, Studio) open and shut from their heading (a disclosure button, a chevron after the label; opening fades the rows in, shutting is instant; saved per device under `loams.shell.nav.collapsed`). In the Pages and Settings lists the arrow keys move between rows, Home and End jump, and on a group heading Right opens and Left shuts; rows show an inset focus ring. No pinned pages shows a quiet dashed note. A new page rises in (fadeUp, `--op-duration-page`, CSS on the page's root, so a same-page update does not animate). The top bar's sidebar, inspector and drawer toggles are `IconButton`s with a tooltip naming the shortcut; the current crumb is 500. The palette's search field draws one ring, its 1px border in `--op-focus` (no outline), and the rows it opens with rise 25ms apart (capped at 8); rows a query brings in just appear. CodeRabbit: hover no longer replaces an invalid field's danger border (`CONTROL_BOX`, `.loams-input`), and a disabled `ListRow` ignores a caller's `onClick`. |
| T14-1 | Task 12's `keyboard.tsx` (`SHELL_KEYMAP`, `run`, the W12 stack) was not on `dev` when Task 14 started; both tasks ran in parallel, and #825 (Task 12) merges first. | `plugins/shell/src/keyboard.tsx`, `test/menu-commands.test.tsx` | 12 | `useShellKeys`'s `run` is hoisted out of the keydown effect (a stable ref over `latest` and `left`), and a second listener runs the menu's shell commands (palette, the three panel toggles, Back, Forward, Settings, Reopen Closed Thread) through it, so a chord and a click share one path. An event already `defaultPrevented` is left alone. `chat.new`, `thread.next` and `thread.jump.N` are not taken: they are Studio's listener's, and `run` would send them back out as `SHELL_COMMAND_EVENT`. The interim `menu-commands.ts` is gone. |
| T14-2 | Small changes to the plan's code. Look Up names at most 40 characters of the selection and adds "…" when it cuts. Check for Updates… also shows the main window, where the update state is drawn. The platform-electron test lives at `src/shell-commands.test.ts`, beside that package's other tests (it has no `test/` folder). `menu-model.test.ts` also pins that no menu item registers its own accelerator, and `tray-model.test.ts`'s Help check reads `show-logs` (the old `logs` id stays an alias in `index.ts` for one release). `menu.electron.ts` also sets each item's `id`, so `Menu.getMenuItemById` finds it. Checked in Electron (Linux, fake engine, dark and light at 1440): the menu bar reads File, Edit, View, Window, Help with the chords shown, and clicking Toggle Sidebar, Toggle Inspector, Command Palette, Settings and Back runs the shell. | `apps/desktop-electron/{src/main,test}`, `web/packages/platform-electron/src` | — | — |
| T14-3 | The two files adapted from dsh-desktop (`src/main/shell/context-menu-template.ts`, `context-menu.electron.ts`) carry the header; `apps/desktop-electron/provenance.json` is Task 15's file. | `apps/desktop-electron` | 15 | Whichever of Tasks 14 and 15 lands second adds the two rows (`adapt`, at `bc732f5`), so `provenance.test.ts` sees every headed file. |
| T13-1 | `apps/desktop-electron/provenance.json` and `LICENSE.dsh-desktop` do not exist yet: Task 15 creates them (W19), and it runs in parallel. | `apps/desktop-electron/provenance.json` | 15 | Task 13's three adapted files carry the `// Adapted from dataelement/dsh-desktop (MIT), <path>.` header now; their rows land with Task 15's file (whichever PR merges second adds them), at `bc732f5` with the upstream SHA-256: `src/main/shell/window-chrome.ts` ← `src/main/index.ts` (`56a899dc…ca0c08`), `src/main/shell/macos-window-backdrop.ts` ← `src/main/macos-window-backdrop.ts` (`377d5ccb…798332`), `src/preload/window-chrome.ts` ← `src/preload/macos-window-chrome.ts` (`46bfd5b3…776ac4`), all `adapt`. |
| T13-2 | The plan's `window-chrome.ts` calls `applyMacosWindowBackdrop`, which imports `macBackdrop` from `window-chrome.ts`: an import cycle. | `src/main/shell/{window-chrome,macos-window-backdrop}.ts` | — | `macos-window-backdrop.ts` holds the swap (`macosBackdrop(s, solid)`, `applyMacosWindowBackdrop(win, solid)`) and takes the solid colour from its caller, as upstream's file did before it hard-coded its own; `window-chrome.ts` exports the plan's `macBackdrop(s, dark)` over it with `CHROME`'s window colour. `ChromeSync` takes a named `ChromeSyncOptions`. |
| T13-3 | Task 7 as built has no `--lc-traffic-top`: the brand row is the top bar's height and starts after `--lc-traffic` (`.lc-shell[data-sidebar="open"] .lc-brand`), so nothing reads it. The plan's macOS drag region was the whole `.lc-side-head`, which since Task 7 also holds the server switcher and New thread. | `apps/console/src/cordis/console.css` | — | `--lc-traffic-top` is not set. On macOS the brand row (`.lc-brand`, beside the traffic lights) is the sidebar's drag region, its links and buttons `no-drag`; the server switcher and New thread stay ordinary controls. |
| T13-4 | The preload compiles with `lib: ["ES2023"]` (no DOM), so `Document` and `window` do not type-check there. | `src/preload/{window-chrome,index}.ts` | — | `mountWindowChrome` takes a structural `ChromeDocument` (the dataset and the two listener methods); `index.ts` reads `document` and `addEventListener` off `globalThis` with that type. `window-chrome.test.ts` gains a preload case (marks the platform, follows `shell:fullscreen`, ignores a non-boolean, stops on unmount). `fake-desktop.ts` gains `onFullscreen`. |
| T13-5 | Step 7 on this Linux machine (Hyprland, Wayland, Electron 44). | — | 17 | Checked in the built app with the fake engine: `html[data-platform="linux"]`; Window Controls Overlay visible, 44px high; the top bar's end pads 12px + the overlay's width (Hyprland draws only Close, 39px); the top bar is `drag`; minimum size 900×640; Dark → Light → Dark through the palette called `setBackgroundColor` and `setTitleBarOverlay` with the window and ink tokens each time. Flipping the OS dark mode under System is covered by `window-chrome.test.ts` (it is not flipped on the owner's session); macOS (traffic lights, vibrancy, backdrop swap) and Windows stay manual steps on the release checklist, as the plan says. |
| T15-1 | The plan's loader canvas (320×240, 2 cells per mark unit) puts the 52-cell mark against the top and bottom edges, so the seed's bubbles rise off the frame. | `apps/desktop-electron/scripts/generate-loader-gifs.mjs`, `build/splash.html` | 15 | The canvas is 320×320, which leaves 14 cells of headroom above the seed; the splash shows the loader at 120px (the mark about 78px), and the status line uses `text-wrap: balance`. gifenc's entry is CommonJS, so the script takes `GIFEncoder` off the default import. A regeneration is byte-identical (checked twice). |
| T15-2 | `CHROME` (Task 13) and `sync.dark` are not on dev yet; Tasks 13 and 15 run in parallel. | `src/main/shell/splash.electron.ts`, `src/main/index.ts` | 15 | The splash keeps its own `SPLASH_CANVAS` with CHROME's canvas values (`#0a0a0a`, `#ffffff`) and takes `nativeTheme.shouldUseDarkColors` for `dark`. Whichever of 13 and 15 lands second switches both to `CHROME.*.canvas` and `sync.dark`. Task 15 landed second and did. |
| T15-3 | `provenance.test.ts` requires the rows to equal the headed files exactly. Task 13's and 14's adapted files (`window-chrome.ts`, `macos-window-backdrop.ts`, `context-menu-template.ts`, `context-menu.electron.ts`, `preload/window-chrome.ts`) are not on dev yet. | `apps/desktop-electron/provenance.json` | 15 | The file has the 8 W19 rows and the 2 new splash rows. Each later task adds the rows for its own headed files (Task 16 the shim's), and the test fails until it does. Every row is `adapt`. Tasks 13 and 14 landed first, so Task 15 added their five rows (upstream SHA-256 at `bc732f5`). |
| T15-4 | The plan shows the main window only from the gate. Three paths would still show it early or late: macOS `activate` at launch, a tray, menu or notification "open" during the splash, and a quit during the splash (the timeout would show the console mid-quit). | `src/main/index.ts` | 15 | `showMainWindow` calls `gate.ready()` first (an explicit ask ends the splash), `activate` is ignored until the gate has shown the window, and `before-quit` disposes the gate. The splash window also gets `secureWindow` (no `window.open`, no navigation). |
| T15-5 | `requestAnimationFrame` in a window created with `show: false` could have been throttled until the 30s timeout. | `web/apps/console/src/cordis/desktop.ts` | 15 | Measured in Electron on Linux: rAF runs in the hidden window, and the console window shows 0.2–0.9s after its first load, with the splash closed. Playwright's Electron launch emulates `prefers-color-scheme: light` by default, so a dark-splash screenshot needs `colorScheme: null`. |
| T16-1 | Small changes to the plan's code. `executeInMainWorld` sends the shim function's source alone, and a bundler may lower class fields to helper calls outside it, so `DesktopNotification` has no field initialisers or static fields (getters and constructor assignments instead); a test pins that its source calls no such helper. The shim keeps upstream's `tag`, `data`, `on*` handlers and `show`/`close`/`error` events (`show` after the bridge resolves), and `requestPermission(callback)`. A click is main's (focus, then the route), so the page's `onclick` is not called. The web edition's kinds and focus rule are `webNotifier` in `notifications/data.ts` (testable without the plugin), and `onArrive` fires once per id from the stream loop, not from a state updater. `apps/desktop-electron/provenance.json` did not exist on `dev` yet (Task 15 creates it); this task adds it with the shim's row, and Task 15's rows merge in. | `apps/desktop-electron/src/preload/notification-shim.ts`, `provenance.json`, `plugins/studio/src/notifications/data.ts` | 15 | The shim's row is `adapt` at `bc732f5`. |
| T16-2 | The plan's toast showed every `error` state for 8 s. The scheduler checks every few hours, so an offline laptop would get "The update failed" each time; it also re-showed an offer the person had closed on every periodic check (each check passes through `checking`, which the plan's code treated as "close"). Adding with the same id merges fields, so `downloading` kept the `Download` button. | `plugins/desktop-settings/src/update-toast.tsx`, `test/update-toast.test.tsx` | — | An error toasts only when it ends a download or an install (the phase before was `downloading` or `ready`); a failed check shows in Settings › Updates as before. `checking` leaves the toast as it is. Closing the toast remembers its phase and version, and the same pair does not show again in this window; a newer version or the next phase does. Each state passes `actionProps` (undefined clears it). The preview's fake updater (`/?desktop`) pushes its states, so Settings › Updates › "Check now" shows the toast there. Settings › Updates follows `update.onState` too (coordinator, 2026-10-19), so a skip or a download started from the toast shows on the card; it still polls while a check or download runs. |
