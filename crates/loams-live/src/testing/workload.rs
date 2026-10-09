//! The reactive checker's workload (LV1 plan Task 1).

/// A seeded workload: `sessions` sessions watching queries over `tables`
/// tables while `ops` mutations run, with `disturb` happening on the way.
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
