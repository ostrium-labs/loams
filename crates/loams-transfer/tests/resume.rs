//! Kill and resume (DQ1 Task 3, §85 §15): the `loams-transfer put` and `get`
//! processes are killed with SIGKILL midway, started again, and must finish
//! with a file byte-identical to the source.
//!
//! The source is sparse with patterned blocks, never materialised in memory
//! or as dense blocks. The bucket and the download live under
//! `CARGO_TARGET_TMPDIR` (not `/tmp`), or `LOAMS_TRANSFER_TEST_DIR`, and are
//! deleted at the end.
//!
//! CI runs 256 MiB. The 10 GiB run is ignored by default:
//! `cargo test -p loams-transfer --test resume -- --ignored`.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use loams_store::Store;
use loams_transfer::net::ServerIdentity;
use loams_transfer::server::{Access, MemNamespaces, StaticTokens};
use loams_transfer::standalone::{self, Listeners, Serve};
use loams_transfer::{MemCheckpoints, TransfersConfig};
use serde_json::Value;
use sha2::{Digest, Sha256};

const TOKEN: &str = "resume-token-0123456789";
const MIB: u64 = 1 << 20;

fn work_dir() -> tempfile::TempDir {
    let base = std::env::var_os("LOAMS_TRANSFER_TEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    std::fs::create_dir_all(&base).expect("base dir");
    tempfile::Builder::new()
        .prefix("dq1-resume-")
        .tempdir_in(base)
        .expect("work dir")
}

/// A sparse file of `size` bytes with a 64 KiB patterned block every 4 MiB
/// (the rest is holes, so it reads as zeros). Each block depends on its
/// offset, so no two chunks are alike.
fn sparse_source(path: &Path, size: u64) {
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .expect("create");
    f.set_len(size).expect("len");
    let mut block = vec![0u8; 64 << 10];
    let mut off = 0u64;
    while off < size {
        let mut x = off ^ 0x9E37_79B9_7F4A_7C15;
        for b in block.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        let n = block.len().min((size - off) as usize);
        std::os::unix::fs::FileExt::write_all_at(&f, &block[..n], off).expect("write");
        off += 4 * MIB;
    }
    f.sync_all().expect("sync");
}

fn sha_file(path: &Path) -> String {
    let mut f = File::open(path).expect("open");
    let mut h = Sha256::new();
    let mut buf = vec![0u8; MIB as usize];
    loop {
        let n = f.read(&mut buf).expect("read");
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    hex::encode(h.finalize())
}

/// `cmp`: equal length and equal bytes, read in blocks.
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (mut fa, mut fb) = (File::open(a).expect("a"), File::open(b).expect("b"));
    if fa.metadata().expect("a").len() != fb.metadata().expect("b").len() {
        return false;
    }
    let (mut x, mut y) = (vec![0u8; MIB as usize], vec![0u8; MIB as usize]);
    loop {
        let n = fa.read(&mut x).expect("read a");
        if n == 0 {
            return true;
        }
        fb.read_exact(&mut y[..n]).expect("read b");
        if x[..n] != y[..n] {
            return false;
        }
    }
}

struct Server {
    _l: Listeners,
    port: u16,
    pin: String,
}

async fn server(bucket: &Path) -> Server {
    std::fs::create_dir_all(bucket).expect("bucket");
    let url = format!("file://{}", bucket.canonicalize().expect("abs").display());
    let store = Store::from_url(&url, std::iter::empty::<(String, String)>()).expect("store");
    let cfg = TransfersConfig {
        min_chunk: MIB,
        ..TransfersConfig::default()
    };
    let auth = StaticTokens::new(Arc::new(MemNamespaces::default()))
        .with(TOKEN, &[Access::Read, Access::Write]);
    let api = standalone::api(store, Arc::new(MemCheckpoints::new()), cfg, Arc::new(auth));
    let (id, _, _) = ServerIdentity::self_signed(&["localhost".into()]).expect("cert");
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
    let l = standalone::bind(
        addr,
        &id,
        standalone::app(api),
        Serve { h3: true, h2: true },
    )
    .await
    .expect("bind");
    Server {
        port: l.addr.port(),
        _l: l,
        pin: id.fingerprint(),
    }
}

fn remote_args(s: &Server, dir: &Path) -> Vec<String> {
    let token = dir.join("token");
    std::fs::write(&token, TOKEN).expect("token");
    vec![
        "--server".into(),
        format!("https://127.0.0.1:{}", s.port),
        "--pin".into(),
        s.pin.clone(),
        "--ns".into(),
        "acme".into(),
        "--token-file".into(),
        token.display().to_string(),
        "--state".into(),
        dir.join("state").display().to_string(),
        "--window".into(),
        "4".into(),
        // Slow enough that the kill lands midway even on a fast machine.
        "--pace-ms".into(),
        "10".into(),
    ]
}

fn spawn(cmd: &str, args: &[String]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_loams-transfer"))
        .arg(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn")
}

/// Reads the child's JSON lines until `stop` says so or it exits; returns
/// every line read.
fn follow(child: &mut Child, mut stop: impl FnMut(&Value) -> bool) -> Vec<Value> {
    let out = child.stdout.take().expect("stdout");
    let mut lines = Vec::new();
    for line in BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let halt = stop(&v);
        lines.push(v);
        if halt {
            break;
        }
    }
    lines
}

fn done_at_least(v: &Value, bytes: u64) -> bool {
    v["event"] == "progress" && v["progress"]["bytes_done"].as_u64().unwrap_or(0) >= bytes
}

/// Runs `cmd` to completion and returns its `done` line and the first
/// progress line.
fn run_to_end(cmd: &str, args: &[String]) -> (Value, Option<Value>) {
    let mut child = spawn(cmd, args);
    let lines = follow(&mut child, |_| false);
    let status = child.wait().expect("wait");
    assert!(status.success(), "{cmd} failed: {lines:?}");
    let done = lines
        .iter()
        .find(|v| v["event"] == "done")
        .cloned()
        .expect("done line");
    let first = lines.iter().find(|v| v["event"] == "progress").cloned();
    (done, first)
}

/// Starts `cmd`, kills it with SIGKILL once `at` bytes are done.
fn kill_midway(cmd: &str, args: &[String], at: u64) {
    let mut child = spawn(cmd, args);
    let lines = follow(&mut child, |v| done_at_least(v, at));
    assert!(
        lines.iter().any(|v| done_at_least(v, at)),
        "{cmd} ended before {at} bytes: {lines:?}"
    );
    child.kill().expect("SIGKILL");
    let _ = child.wait();
}

async fn kill_and_resume(size: u64, chunk: u64) {
    let dir = work_dir();
    let src = dir.path().join("source.bin");
    let dest = dir.path().join("downloaded.bin");
    sparse_source(&src, size);
    let want = tokio::task::spawn_blocking({
        let src = src.clone();
        move || sha_file(&src)
    });
    let s = server(&dir.path().join("bucket")).await;
    let remote = remote_args(&s, dir.path());
    let started = Instant::now();

    // Upload: kill at 40 %, resume, finish.
    let mut put = remote.clone();
    put.extend([
        "--chunk-size".into(),
        chunk.to_string(),
        src.display().to_string(),
    ]);
    let put2 = put.clone();
    tokio::task::spawn_blocking(move || kill_midway("put", &put2, size * 2 / 5))
        .await
        .expect("join");
    let (done, first) = tokio::task::spawn_blocking(move || run_to_end("put", &put))
        .await
        .expect("join");
    assert_eq!(done["resumed"], true, "the second put resumes: {done}");
    let resumed_from = first
        .as_ref()
        .and_then(|v| v["progress"]["resumed_from"].as_u64())
        .unwrap_or(0);
    assert!(
        resumed_from >= size * 2 / 5 - chunk * 4,
        "resumed from {resumed_from}"
    );
    let want = want.await.expect("join");
    assert_eq!(done["file_sha256"], want);
    let tid = done["tid"].as_str().expect("tid").to_string();
    let put_hwm = done["vm_hwm_kib"].as_u64();

    // Download: kill at 40 %, resume, finish.
    let mut get = remote.clone();
    get.extend([
        "--tid".into(),
        tid,
        "--dest".into(),
        dest.display().to_string(),
    ]);
    let get2 = get.clone();
    tokio::task::spawn_blocking(move || kill_midway("get", &get2, size * 2 / 5))
        .await
        .expect("join");
    let (gdone, gfirst) = tokio::task::spawn_blocking(move || run_to_end("get", &get))
        .await
        .expect("join");
    let gfrom = gfirst
        .as_ref()
        .and_then(|v| v["progress"]["resumed_from"].as_u64())
        .unwrap_or(0);
    assert!(gfrom > 0, "the second get resumes: {gfirst:?}");

    let (src2, dest2) = (src.clone(), dest.clone());
    let (got, same) =
        tokio::task::spawn_blocking(move || (sha_file(&dest2), same_bytes(&src2, &dest2)))
            .await
            .expect("join");
    assert_eq!(got, want, "SHA-256 of the download");
    assert!(same, "cmp: the download differs from the source");
    eprintln!(
        "kill_and_resume: {size} bytes in {:.1}s; put peak RSS {:?} KiB, get peak RSS {:?} KiB; upload resumed from {resumed_from}, download from {gfrom}",
        started.elapsed().as_secs_f64(),
        put_hwm,
        gdone["vm_hwm_kib"].as_u64(),
    );
    if let Some(kib) = put_hwm {
        assert!(
            kib < 128 * 1024,
            "upload peak RSS {kib} KiB is over 128 MiB"
        );
    }
    // The work directory (bucket, download, source) goes with `dir`.
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_midway_resumes_byte_identical() {
    kill_and_resume(256 * MIB, MIB).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "10 GiB: run locally (DQ1 Task 3 gate)"]
async fn kill_midway_resumes_byte_identical_10gib() {
    kill_and_resume(10 << 30, 16 * MIB).await;
}
