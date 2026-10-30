//! Job runs end to end (DQ1 Task 4, §85 §11): a runner claims runs over
//! HTTP/3, runs configured jobs, streams their logs and artifacts back; a
//! follower resumes after disconnects with a byte-identical log.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use futures::StreamExt;
use loams_transfer::api::Client;
use loams_transfer::client::{self, StateDir, UploadOpts};
use loams_transfer::keys::new_id;
use loams_transfer::net::{ConnectOpts, Mode, ServerIdentity, Trust};
use loams_transfer::runner::{self, JobSpec, JobsFile, RunnerOpts};
use loams_transfer::standalone::{self, Listeners, Serve};
use loams_transfer::{ClaimedRun, RunOutcome, RunView, TransfersConfig};

const TOKEN: &str = "runner-token-0123456789";

struct Fx {
    _l: Listeners,
    client: Client,
    dir: tempfile::TempDir,
}

async fn fx() -> Fx {
    let cfg = TransfersConfig {
        min_chunk: 64 << 10,
        ..TransfersConfig::default()
    };
    let (api, _) = standalone::mem_api(TOKEN, cfg);
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
    let mut o = ConnectOpts::new(
        format!("https://127.0.0.1:{}", l.addr.port()),
        Trust::Pin(id.fingerprint()),
    );
    o.mode = Mode::H3;
    let client = Client::new(o, "acme", TOKEN).expect("client");
    Fx {
        _l: l,
        client,
        dir: tempfile::Builder::new()
            .prefix("dq1-runner-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("dir"),
    }
}

fn sh(script: &str, cwd: Option<&Path>) -> JobSpec {
    JobSpec {
        argv: vec!["sh".into(), "-c".into(), script.into()],
        cwd: cwd.map(Path::to_path_buf),
        env_allow: vec!["REF".into()],
        env: BTreeMap::new(),
        timeout_s: 60,
        artifacts: vec![],
    }
}

fn opts() -> RunnerOpts {
    RunnerOpts {
        runner_id: "runner-test".into(),
        poll: Duration::from_millis(100),
        renew_every: Duration::from_millis(200),
        flush_every: Duration::from_millis(100),
        ..RunnerOpts::default()
    }
}

async fn claim(f: &Fx, job: &str) -> ClaimedRun {
    f.client
        .claim("runner-test", &[job.to_string()])
        .await
        .expect("claim")
        .expect("a queued run")
}

async fn queue(f: &Fx, job: &str, params: BTreeMap<String, String>) -> String {
    let rid = new_id();
    f.client
        .create_run(&rid, job, params)
        .await
        .expect("create");
    rid
}

fn jobs(list: Vec<(&str, JobSpec)>) -> JobsFile {
    JobsFile {
        jobs: list.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
    }
}

async fn wait_done(f: &Fx, rid: &str) -> RunView {
    for _ in 0..600 {
        let v = f.client.run(rid).await.expect("view");
        if v.done() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("run {rid} did not end");
}

#[tokio::test(flavor = "multi_thread")]
async fn runner_refuses_unknown_job_and_unlisted_params() {
    let f = fx().await;
    let state = StateDir::new(f.dir.path().join("state")).expect("state");
    let rid = queue(&f, "deploy", BTreeMap::new()).await;
    let run = claim(&f, "deploy").await;
    let none = jobs(vec![("ci", sh("true", None))]);
    let out = runner::run_one(&f.client, &state, &none, &opts(), &run)
        .await
        .expect("run");
    assert_eq!(out, RunOutcome::Failed);
    let v = f.client.run(&rid).await.expect("view");
    assert!(v.message.expect("message").contains("no job deploy"));

    let mut p = BTreeMap::new();
    p.insert("SECRET".to_string(), "x".to_string());
    let rid = queue(&f, "ci", p).await;
    let run = claim(&f, "ci").await;
    let ci = jobs(vec![("ci", sh("echo should-not-run", None))]);
    let out = runner::run_one(&f.client, &state, &ci, &opts(), &run)
        .await
        .expect("run");
    assert_eq!(out, RunOutcome::Failed);
    let v = f.client.run(&rid).await.expect("view");
    assert_eq!(v.log_bytes, 0, "the job never started");
}

#[tokio::test(flavor = "multi_thread")]
async fn params_reach_the_job_as_env() {
    let f = fx().await;
    let state = StateDir::new(f.dir.path().join("state")).expect("state");
    let mut p = BTreeMap::new();
    p.insert("REF".to_string(), "main".to_string());
    let rid = queue(&f, "ci", p).await;
    let run = claim(&f, "ci").await;
    let ci = jobs(vec![("ci", sh("printf 'ref=%s' \"$REF\"; exit 3", None))]);
    let out = runner::run_one(&f.client, &state, &ci, &opts(), &run)
        .await
        .expect("run");
    assert_eq!(out, RunOutcome::Failed);
    let v = f.client.run(&rid).await.expect("view");
    assert_eq!(v.exit_code, Some(3));
    let mut log = f.client.read_log(&rid, 0, false).await.expect("log");
    let mut text = Vec::new();
    while let Some(b) = log.bytes.next().await {
        text.extend_from_slice(&b.expect("bytes"));
    }
    assert_eq!(text, b"ref=main");
}

/// What the 50 MB job prints, made the same way here.
fn expected_log(bytes: usize) -> Vec<u8> {
    let line = b"loams-transfer runner log line 0123456789 abcdefghijklmnopqrstuvwxyz\n";
    line.iter().copied().cycle().take(bytes).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_resumes_from_offset() {
    const SIZE: usize = 50 << 20;
    let f = fx().await;
    let state = StateDir::new(f.dir.path().join("state")).expect("state");
    let script = format!(
        "yes 'loams-transfer runner log line 0123456789 abcdefghijklmnopqrstuvwxyz' | head -c {SIZE}"
    );
    let ci = jobs(vec![("ci", sh(&script, None))]);
    let rid = queue(&f, "ci", BTreeMap::new()).await;
    let stop = Arc::new(AtomicBool::new(false));
    let runner_task = {
        let (client, state, stop) = (f.client.clone(), state.clone(), Arc::clone(&stop));
        tokio::spawn(async move { runner::serve(&client, &state, &ci, &opts(), stop).await })
    };

    // A follower that disconnects twice, then resumes to the end from its
    // mirror's length.
    let mirror = f.dir.path().join("mirror.log");
    let mut file = std::fs::File::create(&mirror).expect("mirror");
    let mut have = 0u64;
    for _ in 0..2 {
        loop {
            let mut ls = f.client.read_log(&rid, have, true).await.expect("read");
            let mut got = 0u64;
            while let Some(b) = ls.bytes.next().await {
                let b = b.expect("bytes");
                file.write_all(&b).expect("write");
                got += b.len() as u64;
                if got >= 3 << 20 {
                    break; // drop the stream: a disconnect mid-log
                }
            }
            have += got;
            if got > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    drop(file);
    let v = client::tail::tail(&f.client, &rid, &mirror, |_, _| {})
        .await
        .expect("tail");
    assert_eq!(v.state, "succeeded");
    assert_eq!(v.log_bytes, SIZE as u64);
    let got = std::fs::read(&mirror).expect("read mirror");
    assert_eq!(got.len(), SIZE);
    assert!(
        got == expected_log(SIZE),
        "the mirror is not byte-identical"
    );
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    runner_task.await.expect("join").expect("runner");
}

#[tokio::test(flavor = "multi_thread")]
async fn artifacts_uploaded_and_listed() {
    let f = fx().await;
    let state = StateDir::new(f.dir.path().join("state")).expect("state");
    let work = f.dir.path().join("work");
    std::fs::create_dir_all(&work).expect("work");
    let mut job = sh(
        "mkdir -p out && head -c 300000 /dev/zero | tr '\\0' 'x' > out/report.txt && echo built",
        Some(&work),
    );
    job.artifacts = vec!["out/*.txt".into(), "../../*".into()];
    let ci = jobs(vec![("ci", job)]);
    let rid = queue(&f, "ci", BTreeMap::new()).await;
    let run = claim(&f, "ci").await;
    let out = runner::run_one(&f.client, &state, &ci, &opts(), &run)
        .await
        .expect("run");
    assert_eq!(out, RunOutcome::Succeeded);
    let v = f.client.run(&rid).await.expect("view");
    assert_eq!(
        v.artifacts.len(),
        1,
        "only files inside the job dir: {:?}",
        v.artifacts
    );
    assert_eq!(v.artifacts[0].name, "out/report.txt");
    let dest = f.dir.path().join("report.txt");
    client::download(
        &f.client,
        &v.artifacts[0].tid,
        &dest,
        &UploadOpts::default(),
        |_| {},
    )
    .await
    .expect("download");
    assert_eq!(std::fs::read(&dest).expect("read"), vec![b'x'; 300_000]);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_job() {
    let f = fx().await;
    let state = StateDir::new(f.dir.path().join("state")).expect("state");
    let ci = jobs(vec![("ci", sh("echo started; sleep 30", None))]);
    let rid = queue(&f, "ci", BTreeMap::new()).await;
    let run = claim(&f, "ci").await;
    let task = {
        let (client, state) = (f.client.clone(), state.clone());
        tokio::spawn(async move { runner::run_one(&client, &state, &ci, &opts(), &run).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    f.client.cancel_run(&rid).await.expect("cancel");
    let out = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("stops promptly")
        .expect("join")
        .expect("run");
    assert_eq!(out, RunOutcome::Cancelled);
    let v = wait_done(&f, &rid).await;
    assert_eq!(v.state, "cancelled");
    assert_eq!(v.log_bytes, 8, "the output before the cancel is kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_marks_timed_out() {
    let f = fx().await;
    let state = StateDir::new(f.dir.path().join("state")).expect("state");
    let mut job = sh("sleep 30", None);
    job.timeout_s = 1;
    let ci = jobs(vec![("ci", job)]);
    let rid = queue(&f, "ci", BTreeMap::new()).await;
    let run = claim(&f, "ci").await;
    let out = runner::run_one(&f.client, &state, &ci, &opts(), &run)
        .await
        .expect("run");
    assert_eq!(out, RunOutcome::TimedOut);
    assert_eq!(f.client.run(&rid).await.expect("view").state, "timed_out");
}
