![Loams — Your data. Your bucket.](../docs/assets/loams-banner.svg)

# Loams Applications

This directory contains client applications for the Loams platform.

## Applications

| Application | Path | Technology | Description |
| --- | --- | --- | --- |
| **Loams Desktop** | [`desktop/`](desktop/) | Rust, GPUI, WebKitGTK | Native cross-platform desktop application for Linux, macOS, and Windows. Standalone Cargo workspace. |
| **Loams Mobile** | [`mobile/`](mobile/) | Android (Kotlin, Compose), iOS (Swift, SwiftUI), Go | Native mobile applications for Android and iOS, accompanied by an offline local Go mock service. |

## Documentation

- **Desktop**: See [`desktop/README.md`](desktop/README.md) and [`desktop/native/LOAMS.md`](desktop/native/LOAMS.md).
- **Mobile**: See [`mobile/README.md`](mobile/README.md) and [`mobile/MIGRATION.md`](mobile/MIGRATION.md).
- **Monorepo Architecture**: See [`docs/monorepo.md`](../docs/monorepo.md) for how Nx and pnpm coordinate native builds alongside web and plugins.
