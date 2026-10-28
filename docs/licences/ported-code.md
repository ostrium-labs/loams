# Ported code: headers and the register

Loams ports code from other open-source projects only under licences that
allow it (Apache-2.0, MIT, BSD), never from AGPL, BSL, FSL, SSPL, ELv2 or
the Redis source licences (design 77 section 12, D1649). This page is the
rule for every ported file, and the register of them. Today it covers the KV
gateways (`crates/loams-resp`, `crates/loams-cql`) and the CDC consumer in
`crates/loams-tikv` (plan KV1); other ported code keeps the attribution
recorded in `NOTICE`.

## The header

A ported file starts with one block per origin, then one line saying it is
modified. In Rust:

```rust
// ported-from: https://github.com/tidb-incubator/tidis
// revision: 555856206f248737b12d3f148b10f08a604bc821
// path: src/frame.rs
// licence: Apache-2.0
// ported-from: https://github.com/tokio-rs/mini-redis
// revision: (as carried by Tidis at the revision above)
// path: src/frame.rs
// licence: MIT, Copyright (c) 2020 Tokio Contributors
// Modified by Ostrium Labs for Loams.
```

- `ported-from:` is the origin repository's URL. A file built from several
  origins (a Tidis file that is itself derived from mini-redis) has one
  block per origin.
- `revision:` is the full commit the file was ported from, or, for an
  origin reached through another, a note saying so.
- `path:` names the origin's source file. When a Loams file merges several
  origin files, `path:` lists them, comma-separated.
- `licence:` is the origin's SPDX identifier, and for MIT and BSD the
  copyright line too.
- The four lines come in that order; a block without any of them fails the
  check. The last line of the header is `Modified by Ostrium Labs for
  Loams.` (the case of "modified" is free).

The origin's licence text and NOTICE go in the repository's `NOTICE`, not
in the file.

## The register

Every ported file is listed here, with the origin files it names. The check
fails when a file listed here lacks the header, so a port cannot lose its
attribution in a later edit.

| Loams file | Origin | Origin files |
|---|---|---|
| `crates/loams-resp/src/frame.rs` | Tidis `5558562`, mini-redis (through Tidis) | `src/frame.rs`, `src/parse.rs` |
| `crates/loams-resp/src/server.rs` | Tidis `5558562`, mini-redis (through Tidis) | `src/server.rs`, `src/connection.rs` |

## The check

`scripts/licences/check-ported-headers.sh` runs in CI's `Deps: cargo-deny`
job. It fails when a file under `crates/loams-resp`, `crates/loams-cql` or
`crates/loams-tikv/src/cdc` contains `ported-from:` without the full header,
and when a file the register lists lacks a `ported-from:` block naming each
of its origin files. `--self-test` runs it on planted good and bad files in
`scripts/licences/testdata/`.
