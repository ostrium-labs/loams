//! The ClickHouse dialect front end of the Loams House Rust engine (design §75
//! §6.3; plan HE1 Tasks 22, 24 and 25).
//!
//! Statements are parsed with DataFusion's sqlparser (`ClickHouseDialect`),
//! after a lexical pre-pass for the forms it misreads. A guard then refuses
//! (`48`) or rewrites every construct `SqlToRel` would ignore, ClickHouse's name
//! and type rules are applied, and DataFusion plans the result over the House's
//! catalog.
