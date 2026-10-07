![Loams — Your data. Your bucket.](../../../../docs/assets/loams-banner.svg)

# Local Loams Desktop packaging

Use the native-OS scripts from the standalone workspace root:

- Linux: `bash scripts/package-linux.sh` (set `PROFILE=debug` for a debug artifact).
- macOS: `bash scripts/package-macos.sh`. Produces `Loams Desktop.app`, a DMG, and a bundle archive. Signing and Apple notarization are opt-in; no publishing occurs.
- Windows: `pwsh -NoProfile -File scripts/package-windows.ps1`. Requires MSVC/Windows SDK and Inno Setup 6. Produces a portable zip, executable, and per-user installer.

Artifacts live under `target/package`; Cargo invocations explicitly use the local target directory. Package notices include MIT/upstream attribution, Loams Apache-2.0, font licenses and the Parakeet notice. Icons derive from the existing Loams placeholder mark.

The executable is `loams-desktop`, the application ID is `dev.loams.desktop`, and the only OS URL scheme is `loams`. Installers do not adopt Zeron data or installations. Upgrades are manual: no updater feed config, release manifest, cloud deployment or automatic update is activated. See `../LOAMS.md` for build prerequisites and compatibility.
