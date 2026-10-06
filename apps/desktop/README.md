# Loams Desktop

Loams Desktop is the native desktop application for the Loams platform, built with [GPUI](https://github.com/zed-industries/zed) in Rust.

## Structure

- **[`native/`](native/)**: Standalone Cargo workspace containing the desktop client application, core engine, document sync, UI components, and packaging configurations.
  - See [`native/LOAMS.md`](native/LOAMS.md) for build instructions, platform prerequisites (Linux GPUI/WebKitGTK, macOS, Windows), configuration variables, and policies.
- **[`VALIDATION.md`](VALIDATION.md)**: Import validation report, test results, and Linux verification logs.
- **[`import-provenance.json`](import-provenance.json)**: Source commit provenance and SHA-256 integrity digests for all imported tracked files.
- **[`verify_import.py`](verify_import.py)** / **[`test_verify_import.py`](test_verify_import.py)**: Automated verification suite ensuring desktop crate independence, legal notice integrity, and monorepo isolation.

## Quick Start

From the monorepo root:

```sh
# Run minimal desktop compile check (Linux; browser excluded)
pnpm nx run loams-desktop:check-minimal

# Run focused desktop unit tests
pnpm nx run loams-desktop:test-focused

# Verify import provenance and isolation invariants
pnpm nx run loams-desktop:verify-import

# Build desktop (requires GPUI & WebKitGTK platform libraries)
pnpm build:desktop
```
