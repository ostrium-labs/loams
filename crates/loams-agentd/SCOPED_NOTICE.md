# Desktop import notice

This notice applies only to `apps/desktop/native`, not to the monorepo's core crates.

Imported from the tracked files of `loams-desktop` at commit `c9205f8f949711c3315af551c804c56452d2fbd7`. The original upstream MIT `LICENSE`, `NOTICE`, `THIRD_PARTY_NOTICES.md`, crate licenses, font/icon licenses, and voice-model notice are preserved byte-for-byte. Zeron and its upstream URLs appear in those texts solely for attribution and dependency provenance.

Original notice path mapping:

- `crates/loams-brand` → `crates/loams-desktop-brand`.
- `crates/loams-link` → `crates/loams-desktop-link`.
- `.github/workflows/loams.yml` → not imported; no inherited workflows are activated.
- Mobile/client/text subtrees → not imported after checking the desktop dependency closure.

Desktop import changes: private desktop-specific package/binary names; reuse of the Loams identity and placeholder mark; isolated env/data/service IDs; coordinated CLI/URL/packaging paths; disabled updater/release defaults; optional reduced Linux-browser build; scoped Nx wrappers and verification. See `LOAMS.md` for behavior and explicit protocol compatibility.

Loams-added identity and integration code remains Apache-2.0 under its existing licenses; inherited code remains under its original MIT and third-party licenses. The Apache license is also copied into local package notices. No upstream copyright owner or dependency repository is renamed.
