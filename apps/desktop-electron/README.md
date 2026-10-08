# Loams Desktop (Electron)

The Loams console in an Electron shell with a managed local engine (plan AP1e).
Main and preload are built by electron-vite; the renderer is the console build
(`web/apps/console/dist`).

```sh
pnpm --filter @loams/desktop test
pnpm --filter @loams/desktop typecheck
pnpm --filter @loams/desktop build   # out/main, out/preload
pnpm --filter @loams/desktop dev
```

The engine binary is looked up from `LOAMS_BIN`, the packaged resources, then the
Cargo target directory (`CARGO_TARGET_DIR`, else `cargo metadata`, dev only).
