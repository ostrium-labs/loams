//! Carries FL2 Ruling 2's rpath to this package's binary and test binaries.
//!
//! `loams-chdb-sys` publishes the directory it resolved `libchdb.so` into as
//! `DEP_CHDB_LIB_DIR`, but `cargo:rustc-link-arg` applies only to the package that
//! prints it, so each package that builds a binary linking libchdb re-emits it.
//! `$ORIGIN/../lib` comes first: the image ships `libchdb.so` beside the binary.

fn main() {
    let Ok(lib_dir) = std::env::var("DEP_CHDB_LIB_DIR") else {
        return;
    };
    println!("cargo:rerun-if-env-changed=LIBCHDB_DIR");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
}
