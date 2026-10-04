//! Finds `libchdb.so`, links it, and generates the bindings for the vendored
//! `chdb.h`. FL2 Ruling 2 in full.
//!
//! Ruling 2: *dynamic linking, fetched by digest* — download
//! `linux-<arch>-libchdb.tar.gz` from the `chdb-io/chdb-core` release, check its
//! SHA-256 against a value pinned in this file (never fetched at build time),
//! unpack it into `OUT_DIR`, set an rpath to `$ORIGIN/../lib`, and let
//! `LIBCHDB_DIR` override all of it for offline builds. The digests below were
//! read from the release's own `SHA256SUMS` and re-verified against the bytes
//! Task 0 downloaded (`docs/plans/fl2-dependency-spike.md` §1), which is how
//! the pin stays an immutable reference: a tag is not one, and Task 0's first
//! plain `curl` truncated at 76 MB produced a *mismatching* digest. A mismatch
//! therefore panics and stops the build rather than linking whatever arrived.
//!
//! Ruling 1: the bindings are generated from the header the pinned
//! `libchdb.so` was built from, never hand-written, so `unsafe` appears in
//! `loams-chdb-sys` only as calls into the `bindgen` declarations of the
//! vendored header. `bindgen` needs `libclang` at build time, the way
//! `loams-flow-proto`'s `build.rs` needs `protoc`; set `LIBCLANG_PATH` if
//! libclang is somewhere non-standard.

use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The chDB release FL2 pins. The version string the library reports through
/// `chdb_version()` and `SELECT version()` is a ClickHouse version, recorded
/// with the reference server image in Task 1's `engine.rs`.
const CHDB_VERSION: &str = "26.9.0";

/// The release repository. Task 0 checked this: the plan does not name it, and
/// it is `chdb-io/chdb-core`, not `clickhouse/chdb-core` (`chdb-core` is not a
/// crates.io crate; the `chdb` crate there is an unrelated third-party
/// binding).
const RELEASE_REPO: &str = "chdb-io/chdb-core";

/// Where the archive comes from. Ruling 2 names the asset
/// `linux-<arch>-libchdb.tar.gz`; the release tag is v26.9.0 and `<arch>` is
/// the Debian/Rust name (`x86_64`, `aarch64`) that the asset itself uses.
const RELEASE_URL: &str =
    "https://github.com/chdb-io/chdb-core/releases/download/v26.9.0/linux-{arch}-libchdb.tar.gz";

/// SHA-256 of `linux-x86_64-libchdb.tar.gz` (180 541 166 bytes), from the
/// release's `SHA256SUMS`. FL2 Task 0 §1.
const SHA256_X86_64: &str = "c6398bcc71dc58d12fb81548540aacd5d8248830ec81cec537f51c114670543b";

/// SHA-256 of `linux-aarch64-libchdb.tar.gz`, from the same file. FL2 Task 0 §1.
const SHA256_AARCH64: &str = "070116864cde6fdb3276fb1b28702b3b002da42038bb59c99e2a1360dd7f31ed";

/// The library file inside the archive, and the name it links under.
const LIBRARY: &str = "libchdb.so";

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));

    // The header and the wrapper are the inputs bindgen reads; the digest-pinned
    // library changes only with the pin.
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=chdb.h");
    // Ruling 2's offline override: pointing this at a directory holding
    // `libchdb.so` skips the download and the digest check entirely, which is
    // what CI's cache and an offline build both want.
    println!("cargo:rerun-if-env-changed=LIBCHDB_DIR");
    // bindgen finds libclang through this when it is not on the loader's path.
    println!("cargo:rerun-if-env-changed=LIBCLANG_PATH");

    let lib_dir = match env::var_os("LIBCHDB_DIR") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            let found = dir.join(LIBRARY);
            if !found.is_file() {
                panic!(
                    "LIBCHDB_DIR={} holds no {LIBRARY}; Ruling 2's override expects the \
                     digest-verified library there",
                    dir.display()
                );
            }
            dir
        }
        None => fetch(&out_dir),
    };

    // Where the loader should look at run time. Ruling 2 puts the library next
    // to the binary in the container image, which is `$ORIGIN/../lib` from a
    // binary in `bin/`, and that is the first entry. The resolved directory is
    // the second. Neither propagates to a dependent package's own binaries —
    // `cargo:rustc-link-arg` applies only to the package that prints it — so
    // `loams-chdb/build.rs` re-emits both from `DEP_CHDB_LIB_DIR`, which is what
    // `links = "chdb"` buys.
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // `-l chdb`, not `-l libchdb`: the linker adds the `lib` prefix and `.so`
    // suffix itself, so the flag carries the middle of the file name only.
    let link_name = LIBRARY
        .strip_prefix("lib")
        .and_then(|name| name.strip_suffix(".so"))
        .unwrap_or(LIBRARY);
    println!("cargo:rustc-link-lib=dylib={link_name}");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    // The dependent's build script reads this, so that the rpath reaches the
    // test binaries of `loams-chdb` too.
    println!("cargo::metadata=lib_dir={}", lib_dir.display());

    generate_bindings(&manifest_dir, &out_dir);
}

/// Downloads the pinned archive, fails closed unless it hashes to the pinned
/// digest, and unpacks `libchdb.so` into `out_dir`. Returns the directory
/// holding it.
fn fetch(out_dir: &Path) -> PathBuf {
    let arch = env::consts::ARCH;
    let expected = match arch {
        "x86_64" => SHA256_X86_64,
        "aarch64" => SHA256_AARCH64,
        other => panic!(
            "FL2 pins libchdb {CHDB_VERSION} for x86_64 and aarch64 only, and this build is \
             {other}; set LIBCHDB_DIR to a directory holding the library to build anyway"
        ),
    };
    let url = RELEASE_URL.replace("{arch}", arch);

    let archive = out_dir.join(format!("{arch}-libchdb.tar.gz"));
    // A download inside a build script is unusual enough to be worth saying out
    // loud (Ruling 2's own note); CI caches the archive by digest.
    println!(
        "cargo:warning=downloading {url} (FL2 Ruling 2, chdb {CHDB_VERSION} from {RELEASE_REPO})"
    );

    let mut file = File::create(&archive).expect("creating the archive file in OUT_DIR");
    let mut response = ureq::get(&url)
        .call()
        .unwrap_or_else(|err| panic!("downloading {url} failed: {err}"));
    std::io::copy(&mut response.body_mut().as_reader(), &mut file)
        .unwrap_or_else(|err| panic!("writing {url} to {} failed: {err}", archive.display()));
    file.flush().expect("flushing the downloaded archive");

    let actual = sha256_of(&archive);
    if actual != expected {
        // Fail closed: a tag is not an immutable reference, and Task 0's
        // truncated `curl` produced a mismatching digest, so this is a real
        // failure mode rather than a theoretical one.
        panic!(
            "SHA-256 mismatch for {url}\n  expected {expected} (pinned in build.rs, from the \
             release's SHA256SUMS)\n  actual   {actual}\nRefusing to link an unverified \
             libchdb: delete {} and retry, or set LIBCHDB_DIR to a digest-verified library.",
            archive.display()
        );
    }

    let lib_dir = out_dir.join("libchdb");
    fs::create_dir_all(&lib_dir).expect("creating the library directory in OUT_DIR");
    unpack_library(&archive, &lib_dir);
    if !lib_dir.join(LIBRARY).is_file() {
        panic!(
            "{} holds no {LIBRARY}; the verified v{CHDB_VERSION} archive did not contain it",
            lib_dir.display()
        );
    }
    lib_dir
}

/// Hashes a file without holding it in memory: the archive is 180 MB and the
/// library it unpacks to is 554 MB.
fn sha256_of(path: &Path) -> String {
    let mut file = File::open(path).expect("opening the downloaded archive");
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .expect("reading the downloaded archive");
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    format!("{:x}", hasher.finalize())
}

/// Unpacks `libchdb.so` out of the verified archive, streaming the gzip so the
/// 554 MB never sits in memory.
fn unpack_library(archive: &Path, lib_dir: &Path) {
    let file = File::open(archive).expect("opening the downloaded archive");
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar.entries().expect("reading the libchdb archive as a tar");
    let mut found = false;
    for entry in entries {
        let mut entry = entry.expect("reading a libchdb archive entry");
        let path = entry
            .path()
            .expect("reading a libchdb archive entry path")
            .into_owned();
        // The archive is one file at the root; anything else is a surprise
        // worth failing on rather than writing outside OUT_DIR.
        if path.file_name().map(|n| n == LIBRARY) != Some(true) || path.components().count() != 1 {
            continue;
        }
        let mut out = File::create(lib_dir.join(LIBRARY)).expect("creating libchdb.so");
        std::io::copy(&mut entry, &mut out).expect("unpacking libchdb.so");
        found = true;
    }
    if !found {
        panic!(
            "the verified libchdb archive {} holds no {LIBRARY}",
            archive.display()
        );
    }
}

/// Runs bindgen over the vendored header.
///
/// The allowlists are restricted to the `chdb_` prefix, so nothing from the
/// C standard library bindgen can see leaks into the bindings. Types reachable
/// from an allowlisted function come along whatever their own names are
/// (`local_result_v2`, for the deprecated `chdb_streaming_*` family).
fn generate_bindings(manifest_dir: &Path, out_dir: &Path) {
    let mut builder = bindgen::Builder::default()
        .header(manifest_dir.join("wrapper.h").to_str().expect("utf-8 path"))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // bindgen anchors every allowlist regex (`^` and `$` around it), so the
        // prefix is a `prefix.*` pattern rather than a bare `prefix`.
        .allowlist_function("chdb_.*")
        .allowlist_type("chdb_.*")
        .allowlist_var("CHDB_.*")
        // `missing_debug_implementations` is a workspace warning and CI runs
        // `-D warnings`, so every generated struct needs its Debug.
        .derive_debug(true)
        // The header's doc comments are 1176 lines of upstream prose and the
        // header itself is vendored next to this crate; repeating them in the
        // generated file would only make the drift harder to read.
        .generate_comments(false)
        // bindgen's own layout tests are for the generated code's benefit; the
        // crate's test is the one that matters, `every_measured_export_has_a_binding`.
        .layout_tests(false);
    // clang finds its own builtin headers (`stdint.h`, `stdbool.h`, …) relative
    // to the libclang it loaded, which works for a packaged install and not for
    // a libclang unpacked out of a `.deb` or a tarball. When the build was told
    // where libclang is, pass the resource directory clang would have used
    // explicitly; otherwise a relocated libclang fails on `chdb.h`'s own
    // `#include <stdbool.h>`.
    let libclang_dir = env::var_os("LIBCLANG_PATH").map(PathBuf::from);
    if let Some(includes) = libclang_dir.as_deref().and_then(clang_builtin_includes) {
        builder = builder
            .clang_arg("-isystem")
            .clang_arg(includes.to_string_lossy());
    }
    let bindings = builder
        .generate()
        .expect("bindgen over the pinned chdb.h (is libclang installed? set LIBCLANG_PATH)");

    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("writing the generated bindings into OUT_DIR");
}

/// Looks for clang's builtin include directory beside a libclang directory, in
/// the two shapes an installation has: `<llvm>/lib/clang/<major>/include`
/// directly under it, and `<llvm>/lib/clang/<major>/include` beside it under
/// `..`. Returns `None` for a normal packaged install, where clang finds it by
/// itself and this whole path is not taken.
fn clang_builtin_includes(libclang_dir: &Path) -> Option<PathBuf> {
    let mut roots = vec![libclang_dir.to_path_buf()];
    if let Some(parent) = libclang_dir.parent() {
        roots.push(parent.to_path_buf());
    }
    for root in roots {
        let Ok(versions) = fs::read_dir(root.join("clang")) else {
            continue;
        };
        for version in versions.flatten() {
            let includes = version.path().join("include");
            if includes.join("stdint.h").is_file() {
                return Some(includes);
            }
        }
    }
    None
}
