//! The sidecar protocol (DQ1 Task 3, §85 §12): JSON lines in, JSON lines
//! out, one agent per state directory.

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use loams_transfer::TransfersConfig;
use loams_transfer::net::ServerIdentity;
use loams_transfer::standalone::{self, Serve};
use serde_json::{Value, json};

const TOKEN: &str = "agent-token-0123456789";

fn agent(state: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_loams-transfer"))
        .args(["agent", "--state"])
        .arg(state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn agent")
}

/// Lines from the agent's stdout, on a channel.
fn lines(child: &mut std::process::Child) -> mpsc::Receiver<Value> {
    let out = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    rx
}

fn until(rx: &mpsc::Receiver<Value>, mut f: impl FnMut(&Value) -> bool) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let v = rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("timed out; saw {seen:?}"));
        let hit = f(&v);
        seen.push(v);
        if hit {
            return seen;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_protocol_roundtrip() {
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
    let dir = tempfile::tempdir().expect("dir");
    let src = dir.path().join("in.bin");
    let data: Vec<u8> = (0..(1u32 << 20) + 333).map(|n| (n % 253) as u8).collect();
    std::fs::write(&src, &data).expect("write");
    let state = dir.path().join("state");
    let server =
        json!({"base": format!("https://127.0.0.1:{}", l.addr.port()), "pin": id.fingerprint()});

    let mut child = agent(&state);
    let rx = lines(&mut child);
    let ready = until(&rx, |v| v["event"] == "ready");
    assert!(
        ready[0]["install_id"]
            .as_str()
            .expect("install id")
            .starts_with("install-")
    );

    // A second agent on the same state directory is refused.
    let second = Command::new(env!("CARGO_BIN_EXE_loams-transfer"))
        .args(["agent", "--state"])
        .arg(&state)
        .stdin(Stdio::null())
        .output()
        .expect("second");
    assert!(!second.status.success());

    let mut stdin = child.stdin.take().expect("stdin");
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").expect("write");
        stdin.flush().expect("flush");
    };
    send(
        json!({"op": "upload", "id": "u1", "server": server, "ns": "acme", "token": TOKEN,
                "path": src, "chunk_size": 65536, "window": 4}),
    );
    let seen = until(&rx, |v| {
        v["job"] == "u1" && (v["state"] == "done" || v["state"] == "failed")
    });
    let done = seen.last().expect("last");
    assert_eq!(done["state"], "done", "{seen:?}");
    let tid = done["transfer"]["tid"].as_str().expect("tid").to_string();
    let progress: Vec<&Value> = seen.iter().filter(|v| v["event"] == "progress").collect();
    assert!(!progress.is_empty(), "progress events");
    let last = &progress.last().expect("progress")["progress"];
    assert_eq!(last["bytes_done"], data.len() as u64);
    assert!(last["transport"] == "h3", "{last}");

    let dest = dir.path().join("out.bin");
    send(
        json!({"op": "download", "id": "d1", "server": server, "ns": "acme", "token": TOKEN,
                "tid": tid, "dest": dest}),
    );
    let seen = until(&rx, |v| {
        v["job"] == "d1" && (v["state"] == "done" || v["state"] == "failed")
    });
    assert_eq!(seen.last().expect("last")["state"], "done", "{seen:?}");
    assert_eq!(std::fs::read(&dest).expect("read"), data);

    send(json!({"op": "list", "id": "l1"}));
    let seen = until(&rx, |v| v["id"] == "l1");
    assert_eq!(seen.last().expect("reply")["uploads"], json!([]));

    send(json!({"op": "nonsense"}));
    until(&rx, |v| v["event"] == "error");

    send(json!({"op": "shutdown"}));
    let status = child.wait().expect("wait");
    assert!(status.success());
}
