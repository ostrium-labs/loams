# Loams Desktop Windows package

From the native workspace, run `pwsh -NoProfile -File scripts/package-windows.ps1` on Windows with MSVC, the Windows SDK, and Inno Setup 6.

The package contains `loams-desktop.exe`, legal notices, and bundled-license files. The per-user installer uses its own AppId (`93DB7E9E-5B92-5E45-99A1-105C32A995B8`), installs to the user Programs directory, and registers `loams://` with the executable path quoted. User data lives in `%LOCALAPPDATA%\Loams Desktop` or the explicit `LOAMS_DESKTOP_DATA_DIR` override.

No release URL/config/manifest is shipped; self-updates are disabled. Run a newer local installer to upgrade. `scripts/test-windows-installer.ps1` is destructive to the Loams per-user installation and requires CI or an explicit `-Force` flag. It does not target the upstream Zeron installation.
