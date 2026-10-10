//! `loams-wal`: Loams's WAL service for Neon computes (§28 P4a), without the
//! interpreted sender. `crates/loams-wal-decoder` builds the same command
//! with it (PG2 Task 31); see [`loams_safekeeper::cli`].

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    loams_safekeeper::cli::main(None).await
}
