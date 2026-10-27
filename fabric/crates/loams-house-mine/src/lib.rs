//! `loams-house-mine`: the HE0 mining tool (plan HE1, design §75 §7.1).
//!
//! It reads `system.query_log` dumps and migration files from real
//! applications, writes a feature inventory per application (functions,
//! engines, types, formats, settings, syntax forms, each with its count and
//! the ids of the queries or migration lines it came from), merges the
//! inventories into the House compatibility profile and writes the report.
//!
//! One module per subcommand under [`cmd`], so each subcommand is built and
//! reviewed on its own.

pub mod ast;
pub mod cli;
pub mod cmd;
pub mod explain;
pub mod features;
pub mod inventory;
pub mod lexical;
pub mod matrix;
pub mod oracle;
pub mod profile;
pub mod querylog;
pub mod scrub;
pub mod tiers;
pub mod tokens;
