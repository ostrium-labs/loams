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
    let Role::House(args) = cli.role else {
        panic!("{flags:?}: not the house role");
    };
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
        let (fabric, line) = Self::start_line(flags);
        let addr = line
            .split("http://")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|addr| addr.parse().ok())
            .unwrap_or_else(|| panic!("no address in {line:?}"));
        (fabric, addr)
    }

    /// Starts it and answers its stdout line.
    fn start_line(flags: &[&str]) -> (Self, String) {
        // Tests run side by side: each its own admin port, unless it names one.
        let admin: &[&str] = if flags.contains(&"--admin-listen") {
            &[]
        } else {
            &["--admin-listen", "127.0.0.1:0"]
        };
        let mut child = Command::new(FABRIC)
            .arg("house")
            .args(flags)
            .args(admin)
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
        (Self(child), line)
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

/// `--single-node`'s local catalog (HS1 Task 10): a table made over HTTP is in
/// the SQLite file, its metadata in the bucket under `ns/1/house/`, and it is
/// still there after a restart.
#[test]
fn single_node_keeps_lake_tables() {
    let dir = scratch("lake");
    let worker = worker_binary();
    let flags = [
        "--single-node",
        "--house-listen",
        "127.0.0.1:0",
        "--data-dir",
        dir.to_str().expect("utf-8"),
        "--worker-binary",
        worker.to_str().expect("utf-8"),
    ];
    let post = |addr: SocketAddr, sql: &str| {
        let key = std::fs::read_to_string(dir.join(house::LOCAL_KEY_FILE)).expect("key");
        http(
            addr,
            &format!(
                "POST / HTTP/1.1\r\nHost: house\r\nX-ClickHouse-User: {}\r\n\
                 X-ClickHouse-Key: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sql}",
                house::LOCAL_USER,
                key.trim(),
                sql.len()
            ),
        )
    };
    let (fabric, addr) = Fabric::start(&flags);
    let created = post(
        addr,
        "CREATE TABLE t (a UInt64, b String) ENGINE = MergeTree ORDER BY a",
    );
    assert!(created.starts_with("HTTP/1.1 200"), "{created}");
    drop(fabric);
    assert!(dir.join("catalog.sqlite").is_file());
    let tables = dir.join("bucket/ns/1/house/default");
    let entries: Vec<_> = std::fs::read_dir(&tables)
        .expect("table directory")
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "one table directory under {}",
        tables.display()
    );

    let (fabric, addr) = Fabric::start(&flags);
    let exists = post(addr, "EXISTS TABLE t");
    assert!(
        exists.ends_with("\r\n\r\n1\n") || exists.ends_with("\r\n1\n"),
        "{exists}"
    );
    let shown = post(addr, "SHOW CREATE TABLE t FORMAT TSVRaw");
    assert!(
        shown.contains("CREATE TABLE t (a UInt64, b String)"),
        "{shown}"
    );
    drop(fabric);
}

/// An existing local key that others can read is put back to 0600 before use
/// (PR #398 review).
#[test]
fn existing_local_key_is_tightened() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = scratch("key-mode");
    let path = dir.join(house::LOCAL_KEY_FILE);
    let first = house::local_key(&path).expect("generated");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert_eq!(house::local_key(&path).expect("reused"), first);
    let mode = std::fs::metadata(&path).expect("key").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
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
        password_sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
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
    // TLS may face other hosts, with a certificate (HS1 Task 20); the native
    // protocol's TLS listener uses the TLS listener's certificate (R9.40).
    let refused = resolve(
        &["--house-tls-listen", "0.0.0.0:8443", "--data-dir", data],
        "",
    )
    .expect_err("no certificate");
    assert!(refused.contains("--house-tls-cert"), "{refused}");
    let refused = resolve(
        &["--native-tls-listen", "0.0.0.0:9440", "--data-dir", data],
        "",
    )
    .expect_err("no certificate for the native protocol");
    assert!(refused.contains("--house-tls-cert"), "{refused}");
    let tls = resolve(
        &[
            "--single-node",
            "--house-tls-listen",
            "0.0.0.0:8443",
            "--data-dir",
            data,
        ],
        "",
    )
    .expect("single node makes its own")
    .tls
    .expect("tls");
    assert!(tls.make && tls.cert.starts_with(data), "{tls:?}");

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

/// One HTTPS request to the House's TLS listener, trusting only `ca`; the
/// response as text, or the TLS error.
fn https(addr: SocketAddr, ca: &Path, request: &str) -> Result<String, String> {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, ServerName};
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(ca).expect("the certificate") {
        roots.add(cert.expect("a certificate")).expect("a root");
    }
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = ServerName::try_from("localhost").expect("name");
    let connection = rustls::ClientConnection::new(std::sync::Arc::new(config), name)
        .map_err(|err| err.to_string())?;
    let tcp = TcpStream::connect(addr).map_err(|err| err.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_secs(60)))
        .expect("timeout");
    let mut stream = rustls::StreamOwned::new(connection, tcp);
    stream
        .write_all(request.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut response = String::new();
    match stream.read_to_string(&mut response) {
        Ok(_) => Ok(response),
        // A server that closes without close_notify still answered.
        Err(_) if !response.is_empty() => Ok(response),
        Err(err) => Err(err.to_string()),
    }
}

/// HS1 Task 20: `--single-node` with `--house-tls-listen` makes its own
/// certificate and serves HTTPS beside the loopback plaintext listener; the
/// local key works as `Authorization: Bearer`, which is how the engine's link
/// signs in.
#[test]
fn tls_listener_serves_with_its_certificate() {
    let dir = scratch("tls");
    let worker = worker_binary();
    let data = dir.to_str().expect("utf-8");
    let (_fabric, line) = Fabric::start_line(&[
        "--single-node",
        "--house-listen",
        "127.0.0.1:0",
        "--house-tls-listen",
        "127.0.0.1:0",
        "--data-dir",
        data,
        "--worker-binary",
        worker.to_str().expect("utf-8"),
    ]);
    let tls: SocketAddr = line
        .split("https://")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|addr| addr.parse().ok())
        .unwrap_or_else(|| panic!("no TLS address in {line:?}"));
    let ca = dir.join("tls").join("house.crt");
    let key = std::fs::read_to_string(dir.join("house.key")).expect("the local key");
    let query = |auth: &str| {
        format!(
            "GET /?query=SELECT%201 HTTP/1.1\r\nHost: localhost\r\n{auth}Connection: close\r\n\r\n"
        )
    };

    let ok = https(
        tls,
        &ca,
        &query(&format!("Authorization: Bearer {}\r\n", key.trim())),
    )
    .expect("https");
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.ends_with("\r\n\r\n1\n"), "{ok}");
    let wrong = https(tls, &ca, &query("Authorization: Bearer not-the-key\r\n")).expect("https");
    assert!(
        wrong
            .to_ascii_lowercase()
            .contains("x-clickhouse-exception-code: 516"),
        "{wrong}"
    );
    let none = https(tls, &ca, &query("")).expect("https");
    assert!(
        none.to_ascii_lowercase()
            .contains("x-clickhouse-exception-code: 516"),
        "{none}"
    );

    // Plaintext on the TLS port gets no HTTP answer.
    let mut plain = TcpStream::connect(tls).expect("connect");
    plain
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let _ = plain.write_all(query("").as_bytes());
    let mut answer = Vec::new();
    let _ = plain.read_to_end(&mut answer);
    assert!(
        !answer.starts_with(b"HTTP/1.1"),
        "{}",
        String::from_utf8_lossy(&answer)
    );

    // A client that does not trust this House's certificate is refused.
    let other = scratch("tls-other");
    let (other_cert, other_key) = (other.join("c.pem"), other.join("k.pem"));
    loams_house::tls::ensure_self_signed(&other_cert, &other_key, &["localhost".to_string()])
        .expect("another certificate");
    assert!(https(tls, &other_cert, &query("")).is_err());
}

/// HS1 Task 20 (R20.5): `--principals` maps the link's principals in the
/// single node's namespace; elsewhere they come from `[[house.principals]]`.
/// The link key, dev mode and the audit log are single-node settings.
#[test]
fn principals_link_key_and_audit_resolve() {
    let dir = scratch("principals");
    let data = dir.to_str().expect("utf-8");
    let settings = resolve(
        &[
            "--single-node",
            "--data-dir",
            data,
            "--link-key-file",
            &format!("{data}/link/link.key"),
            "--principals",
            "tailscale:alice@x.com=read+write; apikey:ci=read",
        ],
        "",
    )
    .expect("resolves");
    assert_eq!(settings.principals.len(), 2);
    assert_eq!(settings.principals[0].principal, "tailscale:alice@x.com");
    assert!(settings.principals[0].scopes.write && !settings.principals[0].scopes.ddl);
    assert_eq!(settings.principals[1].namespace, house::LOCAL_NAMESPACE);
    assert!(!settings.dev_unmapped_principals);
    assert_eq!(settings.audit_log, Some(dir.join("audit.log")));

    for (flags, want) in [
        (vec!["--principals", "x=read"], "single-node"),
        (vec!["--link-key-file", "/k"], "single-node"),
        (vec!["--dev-unmapped-principals"], "single-node"),
    ] {
        let mut all = vec!["--data-dir", data];
        all.extend(flags.iter());
        let err = resolve(&all, "").expect_err("refused");
        assert!(err.contains(want) || err.contains("--single-node"), "{err}");
    }
    let err = resolve(
        &[
            "--single-node",
            "--data-dir",
            data,
            "--principals",
            "x=root",
        ],
        "",
    )
    .expect_err("bad scope");
    assert!(err.contains("unknown scope"), "{err}");

    // The file: a principal with tables, in any namespace.
    let settings = resolve(
        &["--data-dir", data],
        "[[house.principals]]\nprincipal = \"apikey:etl\"\nnamespace = 7\nscopes = [\"read\", \"write\"]\ntables = [\"sales.*\"]\n",
    )
    .expect("from the file");
    assert_eq!(settings.principals[0].namespace, 7);
    assert!(
        settings.principals[0]
            .grants
            .as_ref()
            .is_some_and(|g| g.allows("sales", "x") && !g.allows("web", "x"))
    );
    assert!(
        resolve(
            &["--data-dir", data],
            "[[house.principals]]\nprincipal = \"p\"\nnamespace = 1\nscopes = [\"read\"]\ntables = [\"nodot\"]\n",
        )
        .is_err()
    );
}
