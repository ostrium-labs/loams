# Loams Monorepo Tools

This directory contains repository-level validation, structural invariant checkers, and migration tracking utilities.

## Tools

### [`monorepo/`](monorepo/)

- **[`check.py`](monorepo/check.py)**: Source-independent validator for monorepo configuration:
  - Discovers all Nx projects and verifies configuration validity.
  - Ensures required projects exist (`loams-engine`, `loams-contracts`, `loams-desktop`, `mobile-*`, `plugins*`, `@loams/console`).
  - Verifies pinned package manager (`pnpm@11.27.1`) and Nx version (`23.2.1`).
  - Enforces removal of obsolete competing lockfiles (`web/pnpm-lock.yaml`, `plugins/package-lock.json`).
  - Checks for scoped licenses/notices in `apps/desktop`, `apps/mobile`, and `plugins`.
  - Verifies that `apps/desktop/native` retains an independent Cargo workspace and lockfile.
  - Can be run via `pnpm check:monorepo` or `python3 tools/monorepo/check.py`.
- **[`contracts.py`](monorepo/contracts.py)**: Drift validator for canonical app TypeScript bindings generated from `proto/`.
- **[`test_check.py`](monorepo/test_check.py)**: Unit test suite for `check.py`.
- **[`imports.json`](monorepo/imports.json)**: Audit record of imported sub-repositories, revisions, and file counts.
