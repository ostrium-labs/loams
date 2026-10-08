# Task 4 report
Status: DONE. Tests: 99 pass (8 files, new: registry 6, window-state 3); tsc, biome, electron-vite build clean.

Added: servers/registry.ts, servers/ipc.electron.ts (all handlers assertTrustedSender; activate reloads main window), shell/window-state.ts (adapted, attributed), shell/main-window.ts, full preload bridge (all namespaces, unsubscribe wrappers), shared/global.d.ts (Window.loamsDesktop + LoamsDesktopApi export).
Changed: single-instance now returns watchMainWindow(win); reset listener and pending-nav take handler scoped to main window (take returns null for other senders); getWindow = getMainWindow. Protocol returns 503 {code:'engine_not_ready'} for local with url ''. index.ts uses registry.active(), restores/persists bounds (debounced, flush on close).

Concerns: dev default active is `local` (url '') so proxy 503s until Task 6 engine sets registry.setLocalUrl; LOAMS_DESKTOP_SERVER env override removed (demo is fixed 127.0.0.1:8084). Preload version reads LOAMS_DESKTOP_VERSION env (default 0.0.0): needs real wiring. 503 path in handler.electron.ts is untested (electron-bound).
