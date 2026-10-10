//! Carries Ruling 2's rpath to this package's own targets.
//!
//! `loams-chdb-sys` emits `-Wl,-rpath,$ORIGIN/../lib` and the resolved library
//! directory from its own build script, but `cargo:rustc-link-arg` does not
//! propagate out of the package that prints it: it applies to that package's
//! binaries, examples, benches and tests, and not to a dependent's. Without
//! this file the test binaries here link against `libchdb.so` and then fail to
//! find it at run time — `error while loading shared libraries: libchdb.so`.
//!
//! `loams-chdb-sys` declares `links = "chdb"` and publishes the directory it
//! resolved as build-script metadata, so `DEP_CHDB_LIB_DIR` is where Ruling 2's
//! library actually is: `LIBCHDB_DIR` when it is set, and `OUT_DIR/libchdb`
//! when the build fetched and verified the pinned release.

fn main() {
    let lib_dir = std::env::var("DEP_CHDB_LIB_DIR")
        .expect("loams-chdb-sys declares links = \"chdb\" and publishes DEP_CHDB_LIB_DIR");
    println!("cargo:rerun-if-env-changed=LIBCHDB_DIR");
    // Ruling 2's own entry first: the container image ships `libchdb.so` beside
    // the binary, which is `$ORIGIN/../lib` from `bin/`.
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
}
