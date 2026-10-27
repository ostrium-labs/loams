//! `loams-fabric`: the Fabric's one binary, one role per subcommand (design §32,
//! §49 §3).
//!
//! FL1 has not built the Fabric yet (HS1 R0.2), so the roles are **`house`**:
//! the Loams House front (§49), which links no libchdb (D761) and runs
//! ClickHouse SQL in sealed `loams-house-worker` processes; and **`connect`**
//! (CC1 Task 1, D1821): native connector instances on a BYOC VM, over the
//! engine's streams ([`connect`]). FL1's `ingest` and `flow` roles join
//! [`cli::Role`] when FL1 lands.
//!
//! [`cli`] is the command line, [`house`] turns flags, the `--config` file and
//! `--single-node`'s defaults into the role's settings and starts it.

pub mod cli;
pub mod connect;
pub mod house;
