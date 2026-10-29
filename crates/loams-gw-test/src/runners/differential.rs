//! Differential runs (design §77 §12.3): one generated command stream sent
//! to `loams-resp` and to a reference (Valkey; Titan if KV0 Task 10 finds it
//! runs), the replies compared, and every difference either a bug or a
//! documented deviation. The backend comparison of this crate's
//! [`Scenario`](crate::Scenario)s is the same idea one layer down. Built in
//! KV2.

use super::{Runner, Status};

pub const RUNNER: Runner = Runner {
    name: "differential",
    about: "one command stream to loams-resp and to Valkey, replies compared",
    status: Status::Planned { milestone: "KV2" },
};
