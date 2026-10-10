//! Container tests are `#[ignore]` unless `LOAMS_IT_SQLDB=1` (plan SQ1,
//! Global Constraints). The cfg `loams_it_sqldb` carries that into tests.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(loams_it_sqldb)");
    println!("cargo::rerun-if-env-changed=LOAMS_IT_SQLDB");
    if std::env::var("LOAMS_IT_SQLDB").as_deref() == Ok("1") {
        println!("cargo::rustc-cfg=loams_it_sqldb");
    }
}
