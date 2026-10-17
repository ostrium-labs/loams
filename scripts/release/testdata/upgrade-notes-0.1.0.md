### Upgrading

From **0.1.0-preview.2** (one step at a time): `loams upgrade --to 0.1.0`.

It takes a snapshot first, and `loams upgrade rollback` restores it.

| Migration | Store | Class | What it does |
|---|---|---|---|
| `0000-adopt-preview-2` | meta | Reversible | Record preview.2's versions (preview.2 writes no stack record) |
| `0001-safekeeper-volume` | host | Reversible | Give the safekeeper a volume (after a drain) |

Third-party bumps: none.
Free space needed: 1.5× the snapshot estimate (`loams upgrade check` shows it).
Expected downtime: about 5 minutes, excluding image pulls.
