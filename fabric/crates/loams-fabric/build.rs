//! Carries FL2 Ruling 2's rpath to this binary when it is built with
//! `inproc-worker`, which links libchdb into the front (`--workers=inproc`).
//! Without the feature there is no `loams-chdb-sys` in the graph, no
//! `DEP_CHDB_LIB_DIR`, and nothing to do.

fn main() {
    let Ok(lib_dir) = std::env::var("DEP_CHDB_LIB_DIR") else {
        return;
    };
    println!("cargo:rerun-if-env-changed=LIBCHDB_DIR");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
}
