//! Carries FL2 Ruling 2's rpath to this package's own targets.
//!
//! `loams-chdb-sys` emits the resolved `libchdb.so` directory as build-script
//! metadata, but `cargo:rustc-link-arg` applies only to the package that prints it,
//! so a test binary in this crate links against `libchdb.so` and then fails to find
//! it at run time. `loams-chdb`'s own `build.rs` does the same for its targets; the
//! House needs its own copy because it is a different package. Ruling 2's
//! `$ORIGIN/../lib` comes first, because the container image ships `libchdb.so`
//! beside the binary.

fn main() {
    // `loams-chdb-sys` declares `links = "chdb"` and publishes the directory it
    // resolved: `LIBCHDB_DIR` when it is set, the fetched release's own directory
    // in `OUT_DIR` when it is not.
    let Ok(lib_dir) = std::env::var("DEP_CHDB_LIB_DIR") else {
        // A build of this crate that does not go through `loams-chdb-sys` has
        // nothing to point at; `loams-chdb` fails closed on the same missing piece.
        return;
    };
    println!("cargo:rerun-if-env-changed=LIBCHDB_DIR");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
}
