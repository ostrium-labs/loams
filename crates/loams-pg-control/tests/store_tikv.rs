//! `PgControlStore`'s conformance on the TiKV backend (feature `tikv`), on
//! the metastore's test keyspace. Each case prints `skipped:` and passes
//! when `LOAMS_TEST_PD` is unset.

loams_pg_control::pg_control_store_conformance!(
    loams_pg_control::store::conformance::tikv_factory()
);
