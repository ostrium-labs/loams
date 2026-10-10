//! `loams-fabric house` (HS1 Task 7): the role starts and serves, `--single-node`
//! has §49 §17's defaults, and a plaintext listener off loopback is refused.
//!
//! The binary tests start real workers, so they need `loams-house-worker` built
//! beside `loams-fabric` (`cargo test --workspace` builds it; on its own,
//! `cargo build -p loams-house-worker` first).

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use clap::Parser as _;
use loams_fabric::cli::{Cli, HouseArgs, Role};
use loams_fabric::house::{self, HouseSettings};
use loams_house::{CatalogSpec, HouseFile, SandboxMode, WorkersMode};

const FABRIC: &str = env!("CARGO_BIN_EXE_loams-fabric");

/// FL2's refusal of a plaintext listener off loopback (D111).
fn loopback_message(addr: &str) -> String {
    format!(
        "house listen on {addr}: only loopback addresses are served until the unified auth \
         plan (D111)"
    )
}

fn args(flags: &[&str]) -> HouseArgs {
    let cli = Cli::try_parse_from(["loams-fabric", "house"].iter().chain(flags))
        .unwrap_or_else(|err| panic!("{flags:?}: {err}"));
    let Role::House(args) = cli.role;
    args
}

fn resolve(flags: &[&str], file: &str) -> Result<HouseSettings, String> {
    let file = HouseFile::parse(file).map_err(|err| err.message().to_string())?;
    house::resolve(&args(flags), file).map_err(|err| err.message().to_string())
}

/// A private directory under the target directory, never `/tmp`.
fn scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("loams-fabric")
        .join(format!("{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

fn worker_binary() -> PathBuf {
    let path = Path::new(FABRIC).with_file_name(house::WORKER_BINARY);
    assert!(
        path.is_file(),
        "{} is not built: run `cargo build -p loams-house-worker` (or `cargo test --workspace`)",
        path.display()
    );
    path
}

/// `loams-fabric house …` with stdout piped, killed on drop.
struct Fabric(Child);

impl Fabric {
    fn start(flags: &[&str]) -> (Self, SocketAddr) {
        let mut child = Command::new(FABRIC)
            .arg("house")
            .args(flags)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("loams-fabric starts");
        let stdout = child.stdout.take().expect("stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("the address line");
        let addr = line
            .split("http://")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|addr| addr.parse().ok())
            .unwrap_or_else(|| panic!("no address in {line:?}"));
        (Self(child), addr)
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One HTTP/1.1 request; the whole response as text.
fn http(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("timeout");
    stream.write_all(request.as_bytes()).expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    response
}

#[test]
fn house_role_starts_and_serves_ping() {
    let dir = scratch("ping");
    let worker = worker_binary();
    let (fabric, addr) = Fabric::start(&[
        "--single-node",
        "--house-listen",
        "127.0.0.1:0",
        "--data-dir",
        dir.to_str().expect("utf-8"),
        "--worker-binary",
        worker.to_str().expect("utf-8"),
    ]);

    let ping = http(
        addr,
        "GET /ping HTTP/1.1\r\nHost: house\r\nConnection: close\r\n\r\n",
    );
    assert!(ping.starts_with("HTTP/1.1 200"), "{ping}");
    assert!(ping.ends_with("\r\n\r\nOk.\n"), "{ping}");

    // The single node's generated key reaches a sealed worker.
    let key_file = dir.join(house::LOCAL_KEY_FILE);
    let key = std::fs::read_to_string(&key_file).expect("the local key");
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(&key_file)
        .expect("key")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the key is the owner's only");
    let select = http(
        addr,
        &format!(
            "GET /?query=SELECT%201 HTTP/1.1\r\nHost: house\r\nX-ClickHouse-User: {}\r\n\
             X-ClickHouse-Key: {}\r\nConnection: close\r\n\r\n",
            house::LOCAL_USER,
            key.trim()
        ),
    );
    assert!(select.starts_with("HTTP/1.1 200"), "{select}");
    assert!(
        select.ends_with("\r\n\r\n1\n") || select.ends_with("\r\n1\n"),
        "{select}"
    );
    let refused = http(
        addr,
        "GET /?query=SELECT%201 HTTP/1.1\r\nHost: house\r\nX-ClickHouse-User: default\r\n\
         X-ClickHouse-Key: wrong\r\nConnection: close\r\n\r\n",
    );
    assert!(!refused.starts_with("HTTP/1.1 200"), "{refused}");
    drop(fabric);
}

#[test]
fn single_node_defaults() {
    let dir = scratch("defaults");
    let data = dir.to_str().expect("utf-8");
    let node = resolve(&["--single-node", "--data-dir", data], "").expect("resolves");
    assert!(node.single_node);
    assert_eq!(node.http.listen, "127.0.0.1:8123".parse().expect("addr"));
    assert_eq!(
        node.native_listen,
        Some("127.0.0.1:9000".parse().expect("addr"))
    );
    assert_eq!(node.admin_listen, "127.0.0.1:8125".parse().expect("addr"));
    assert_eq!(node.data_dir, dir);
    assert_eq!(node.http.tmp_dir, dir.join("spool"));
    assert_eq!(
        node.catalog,
        Some(CatalogSpec::Local(dir.join("catalog.sqlite")))
    );
    assert_eq!(
        node.store,
        Some(format!("file://{}", dir.join("bucket").display()))
    );
    assert_eq!(node.local_key, Some(dir.join(house::LOCAL_KEY_FILE)));
    assert_eq!(node.workers, WorkersMode::Process);
    assert_eq!(node.sandbox, loams_house::sandbox::default_mode());
    // Beside the running binary (here, the test binary).
    assert_eq!(
        node.worker_binary,
        std::env::current_exe()
            .expect("exe")
            .with_file_name(house::WORKER_BINARY)
    );

    // Without --single-node: no local catalog, store or key, and no native
    // listener named.
    let server = resolve(&["--data-dir", data], "").expect("resolves");
    assert!(!server.single_node);
    assert_eq!(server.native_listen, None);
    assert_eq!(server.catalog, None);
    assert_eq!(server.store, None);
    assert_eq!(server.local_key, None);

    // The file can ask for single-node; a flag beats the file; a configured user
    // means no generated key.
    let filed = resolve(
        &["--house-listen", "127.0.0.1:18124", "--data-dir", data],
        r#"
        [house]
        single_node = true
        listen = "127.0.0.1:18123"
        [[house.users]]
        user = "alice"
        password_sha256 = "00"
        namespace = 3
        [house.pool]
        min_idle_workers = 1
        [house.catalog]
        url = "rest:http://127.0.0.1:8181/catalog"
        "#,
    )
    .expect("resolves");
    assert!(filed.single_node);
    assert_eq!(filed.http.listen, "127.0.0.1:18124".parse().expect("addr"));
    assert_eq!(filed.local_key, None);
    assert_eq!(filed.pool.min_idle_workers, 1);
    assert_eq!(
        filed.catalog,
        Some(CatalogSpec::Rest("http://127.0.0.1:8181/catalog".into()))
    );

    // --workers=inproc is single-node only; --sandbox=none must be forced.
    let refused = resolve(&["--workers", "inproc", "--data-dir", data], "")
        .expect_err("inproc off a single node");
    assert!(refused.contains("--single-node only"), "{refused}");
    if cfg!(target_os = "linux") {
        let refused = resolve(&["--sandbox", "none", "--data-dir", data], "")
            .expect_err("none must be forced");
        assert!(refused.contains("--unsafe-no-sandbox"), "{refused}");
        let forced = resolve(
            &[
                "--sandbox",
                "none",
                "--unsafe-no-sandbox",
                "--data-dir",
                data,
            ],
            "",
        )
        .expect("forced");
        assert_eq!(forced.sandbox, SandboxMode::None);
    }
}

#[test]
fn non_loopback_plaintext_refused() {
    let dir = scratch("loopback");
    let data = dir.to_str().expect("utf-8");
    for (flag, addr) in [
        ("--house-listen", "0.0.0.0:8123"),
        ("--house-listen", "[::]:8123"),
        ("--native-listen", "10.0.0.1:9000"),
        ("--admin-listen", "0.0.0.0:8125"),
    ] {
        let refused = resolve(&[flag, addr, "--data-dir", data], "").expect_err(addr);
        assert_eq!(refused, loopback_message(addr), "{flag} {addr}");
    }
    let refused = resolve(&["--data-dir", data], "[house]\nlisten = \"0.0.0.0:8123\"")
        .expect_err("from the file too");
    assert_eq!(refused, loopback_message("0.0.0.0:8123"));
    let refused = resolve(
        &["--house-tls-listen", "0.0.0.0:8443", "--data-dir", data],
        "",
    )
    .expect_err("no TLS yet");
    assert!(refused.contains("HS1 Task 20"), "{refused}");

    // The binary refuses before it starts any worker, and says why.
    let output = Command::new(FABRIC)
        .args([
            "house",
            "--house-listen",
            "0.0.0.0:8123",
            "--data-dir",
            data,
        ])
        .env_clear()
        .output()
        .expect("loams-fabric runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&loopback_message("0.0.0.0:8123")),
        "{stderr}"
    );
    assert!(output.stdout.is_empty(), "nothing was served");
}

/// D761 at the binary (HS1 R2.11, Task 2 review M9): beside
/// `no_libchdb_in_front`'s dependency-graph check, the built `loams-fabric` has no
/// `NEEDED libchdb.so`. Skipped under `inproc-worker`, which links it on purpose.
#[test]
fn front_binary_needs_no_libchdb() {
    if cfg!(feature = "inproc-worker") {
        return;
    }
    let needed = |binary: &Path| -> Vec<String> {
        let output = Command::new("readelf")
            .args(["-d", "--wide"])
            .arg(binary)
            .output()
            .expect("readelf (binutils) runs");
        assert!(output.status.success(), "readelf -d {}", binary.display());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains("(NEEDED)"))
            .filter_map(|line| line.split('[').nth(1))
            .map(|lib| lib.trim_end_matches(']').to_string())
            .collect()
    };
    let front = needed(Path::new(FABRIC));
    assert!(
        front.iter().any(|lib| lib.starts_with("libc.so")),
        "{front:?}"
    );
    assert!(
        !front.iter().any(|lib| lib.starts_with("libchdb")),
        "loams-fabric links libchdb (D761 puts it in the worker only): {front:?}"
    );
    // The control: the worker does need it.
    assert!(
        needed(&worker_binary())
            .iter()
            .any(|lib| lib == "libchdb.so"),
        "the check must see libchdb where it is"
    );
}
