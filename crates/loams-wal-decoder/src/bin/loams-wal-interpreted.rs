//! `loams-wal-interpreted`: `loams-wal` with the in-process interpreted sender (PG2 Task 31): the
//! pageserver reads WAL from it directly, decoded by the Neon fork's
//! `wal_decoder`. The command line is `loams_safekeeper::cli`'s.

use std::sync::Arc;

use loams_safekeeper::send::Interpreter;
use loams_wal_decoder::NeonInterpreter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    loams_safekeeper::cli::main(Some(Interpreter(Arc::new(NeonInterpreter)))).await
}
