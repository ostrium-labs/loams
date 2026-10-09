![Loams — Your data. Your bucket.](../docs/assets/loams-banner.svg)

# Loams Applications

This directory contains client applications for the Loams platform.

## Applications

| Application | Path | Technology | Description |
| --- | --- | --- | --- |
| **Loams Desktop** | [`desktop-electron/`](desktop-electron/) | Electron, TypeScript; Rust daemon in `crates/loams-agentd*` | The desktop application for Linux, macOS, and Windows, with its per-user agent daemon (design 50). |
| **Loams Mobile** | [`mobile/`](mobile/) | Android (Kotlin, Compose), iOS (Swift, SwiftUI), Go | Native mobile applications for Android and iOS, accompanied by an offline local Go mock service. |

## Documentation

- **Desktop**: See [`desktop-electron/README.md`](desktop-electron/README.md) and [design 50](../docs/design/50-loams-desktop-daemon.md).
- **Mobile**: See [`mobile/README.md`](mobile/README.md) and [`mobile/MIGRATION.md`](mobile/MIGRATION.md).
- **Monorepo Architecture**: See [`docs/monorepo.md`](../docs/monorepo.md) for how Nx and pnpm coordinate native builds alongside web and plugins.
