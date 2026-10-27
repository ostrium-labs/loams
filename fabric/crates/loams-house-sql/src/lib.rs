//! The ClickHouse dialect front end of the Loams House Rust engine (design §75
//! §6.3; plan HE1 Tasks 22, 24 and 25).
//!
//! [`prepare`] turns a statement's text into a DataFusion logical plan:
//!
//! 1. query parameters are bound as ClickHouse binds them ([`params`]);
//! 2. a lexical pre-pass takes out what sqlparser misreads (`FINAL`, join
//!    strictness, `GLOBAL`, `t.1`) and records it ([`lex`]);
//! 3. sqlparser's `ClickHouseDialect` (DataFusion's 0.62) parses the rest;
//! 4. the guard refuses with `48` every construct `SqlToRel` would ignore,
//!    resolves function names as ClickHouse does, and names output columns as
//!    ClickHouse names them ([`guard`], [`functions`], [`names`]);
//! 5. `SqlToRel` plans over the House's catalog, functions and ClickHouse
//!    types ([`context`]).

pub mod coerce;
pub mod context;
pub mod functions;
pub mod guard;
pub mod lex;
pub mod names;
pub mod params;
pub mod plan;
pub mod rewrite;
pub mod scope;
pub mod temporal;
pub mod untuple;

pub use context::{Catalog, HouseContext, Registry};
pub use guard::{Resolution, TableRead};
pub use plan::{Guarded, Prepared, guard, prepare};
