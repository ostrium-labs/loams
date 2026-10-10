//! The reactive checker's workload (LV1 plan Task 1), and the sizes both
//! seeded checkers read from the environment (Tasks 1 and 2).

use loams_kv::Backend;

/// Seeds `reactive_checker_passes_seeded_workload` runs (default: 10 on the
/// embedded store, 2 on TiKV, as pull requests run them; the nightly TiKV
/// job sets 10). `live_histories_are_snapshot_isolated` reads it too, with
/// its own defaults ([`SizeDefaults::TXN`]).
pub const SEEDS_ENV: &str = "LOAMS_CHECKER_SEEDS";
/// Ops per seed (default 2 000).
pub const OPS_ENV: &str = "LOAMS_CHECKER_OPS";
/// The first seed (default 0); the nightly job derives it from the date so
/// that each night covers new seeds.
pub const SEED_OFFSET_ENV: &str = "LOAMS_CHECKER_SEED_OFFSET";

/// The seeded checker run's sizes: seeds `offset..offset + seeds`, `ops`
/// each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    pub seeds: u64,
    pub ops: usize,
    pub offset: u64,
}

impl Sizes {
    /// The reactive checker's sizes on `backend`, from the environment.
    pub fn from_process_env(backend: Backend) -> Result<Self, String> {
        Self::from_env(backend, &|name| std::env::var(name).ok())
    }

    /// The reactive checker's sizes on `backend`, reading variables through
    /// `get` (unset or empty: the default).
    pub fn from_env(
        backend: Backend,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        Self::from_env_or(backend, SizeDefaults::REACTIVE, get)
    }

    /// The sizes on `backend` from the same variables, with `defaults` for
    /// the unset ones (the transaction checker's are
    /// [`SizeDefaults::TXN`]).
    pub fn from_env_or(
        backend: Backend,
        defaults: SizeDefaults,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        fn read<T: std::str::FromStr>(
            get: &dyn Fn(&str) -> Option<String>,
            name: &str,
            default: T,
        ) -> Result<T, String> {
            match get(name).filter(|v| !v.trim().is_empty()) {
                None => Ok(default),
                Some(v) => v
                    .trim()
                    .parse()
                    .map_err(|_| format!("{name}={v:?} is not a number")),
            }
        }
        let default_seeds = match backend {
            Backend::Embedded => defaults.embedded_seeds,
            Backend::Tikv => defaults.tikv_seeds,
        };
        Ok(Sizes {
            seeds: read(get, SEEDS_ENV, default_seeds)?,
            ops: read(get, OPS_ENV, defaults.ops)?,
            offset: read(get, SEED_OFFSET_ENV, 0)?,
        })
    }
}

/// A checker's sizes when the environment sets none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeDefaults {
    pub embedded_seeds: u64,
    pub tikv_seeds: u64,
    pub ops: usize,
}

impl SizeDefaults {
    /// The reactive checker (LV1 plan row T1-4): 10 seeds on the embedded
    /// store, 2 on TiKV, 2 000 ops each.
    pub const REACTIVE: SizeDefaults = SizeDefaults {
        embedded_seeds: 10,
        tikv_seeds: 2,
        ops: 2_000,
    };

    /// The transaction checker (LV1 plan Task 2): 5 seeds on the embedded
    /// store, 2 on TiKV, 1 000 transactions each.
    pub const TXN: SizeDefaults = SizeDefaults {
        embedded_seeds: 5,
        tikv_seeds: 2,
        ops: 1_000,
    };
}

/// A seeded workload: `sessions` sessions watching queries over `tables`
/// tables while `ops` mutations run, with `disturb` happening on the way.
/// `seed` reproduces the op mix (the sessions' queries and each writer's
/// sequence of ops), not the interleaving of writers, ticks and sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workload {
    pub seed: u64,
    pub sessions: usize,
    pub tables: usize,
    pub ops: usize,
    pub disturb: Vec<Disturbance>,
}

/// Something that happens at op `at_op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disturbance {
    Deploy { at_op: usize },
    Rollback { at_op: usize },
    AddIndex { at_op: usize },
    DropIndex { at_op: usize },
    IdentityChange { at_op: usize },
    NodeKill { at_op: usize },
    Disconnect { at_op: usize },
    DropInvalidation { at_op: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn checker_sizes_default_per_backend() {
        let none = env(&[]);
        assert_eq!(
            Sizes::from_env(Backend::Embedded, &none),
            Ok(Sizes {
                seeds: 10,
                ops: 2_000,
                offset: 0
            })
        );
        assert_eq!(
            Sizes::from_env(Backend::Tikv, &none),
            Ok(Sizes {
                seeds: 2,
                ops: 2_000,
                offset: 0
            })
        );
    }

    #[test]
    fn checker_sizes_come_from_the_environment() {
        let set = env(&[
            (SEEDS_ENV, "10"),
            (OPS_ENV, "500"),
            (SEED_OFFSET_ENV, "20370"),
        ]);
        assert_eq!(
            Sizes::from_env(Backend::Tikv, &set),
            Ok(Sizes {
                seeds: 10,
                ops: 500,
                offset: 20_370
            })
        );
        // Empty is unset; anything else must be a number.
        let empty = env(&[(SEEDS_ENV, "")]);
        assert_eq!(
            Sizes::from_env(Backend::Tikv, &empty).map(|s| s.seeds),
            Ok(2)
        );
        let bad = env(&[(OPS_ENV, "lots")]);
        assert!(Sizes::from_env(Backend::Embedded, &bad).is_err());
    }

    #[test]
    fn txn_checker_sizes_have_their_own_defaults() {
        let none = env(&[]);
        let txn = |backend| Sizes::from_env_or(backend, SizeDefaults::TXN, &none);
        assert_eq!(
            txn(Backend::Embedded),
            Ok(Sizes {
                seeds: 5,
                ops: 1_000,
                offset: 0
            })
        );
        assert_eq!(txn(Backend::Tikv).map(|s| s.seeds), Ok(2));
        let set = env(&[(SEEDS_ENV, "10"), (SEED_OFFSET_ENV, "7")]);
        assert_eq!(
            Sizes::from_env_or(Backend::Tikv, SizeDefaults::TXN, &set),
            Ok(Sizes {
                seeds: 10,
                ops: 1_000,
                offset: 7
            })
        );
    }
}
