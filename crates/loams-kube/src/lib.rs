//! Shared controller runtime of the Loams operator (design 86, plan OP2-A). Preview.
//!
//! Today this crate holds leader election with a self-fence ([`lease`]).

pub mod lease;

pub use lease::{
    Clock, KubeLeaseStore, LeaderExit, LeaderToken, LeaseConfig, LeaseError, LeaseStore,
    MemLeaseStore, TokioClock, run_leader,
};
