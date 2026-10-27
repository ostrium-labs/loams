//! `loams knowledge publish` and `verify` (MS1 Task 9): argument parsing,
//! output, and the commands over the mock Walrus and a `file://` bucket.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use loams_knowledge::cli::{Deps, KnowledgeArgs, KnowledgeCommand, Output, VisibilityArg, execute};
use loams_knowledge::{KnowledgeError, KnowledgeManifest, Layout};
use support::{FakeSigner, FakeVerifier, MockWalrus, REPO, SIGNER};

/// The `loams` binary's shape: `loams knowledge …`.
#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Top,
}

#[derive(Debug, Subcommand)]
enum Top {
    Knowledge(KnowledgeArgs),
}

fn parse(args: &[&str]) -> Result<KnowledgeArgs, clap::Error> {
    let mut all = vec!["loams", "knowledge"];
    all.extend_from_slice(args);
    Cli::try_parse_from(all).map(|cli| match cli.command {
        Top::Knowledge(k) => k,
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// A public repository with a commit, a `file://` bucket and a config.
struct Fixture {
    walrus: MockWalrus,
    _dir: tempfile::TempDir,
    repo: std::path::PathBuf,
    bucket_dir: std::path::PathBuf,
    bucket: String,
    config: std::path::PathBuf,
    commit: String,
}

impl Fixture {
    async fn new(public: bool) -> Self {
        let walrus = MockWalrus::start().await;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".loams")).unwrap();
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(
            repo.join(".loams/knowledge.toml"),
            format!("public = {public}\n"),
        )
        .unwrap();
        std::fs::write(repo.join("README.md"), "# Fixture\n").unwrap();
        std::fs::write(repo.join("docs/a.md"), "doc a\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "one"]);
        let commit = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        let bucket_dir = dir.path().join("bucket");
        std::fs::create_dir_all(&bucket_dir).unwrap();
        let bucket = url::Url::from_directory_path(&bucket_dir)
            .unwrap()
            .to_string();
        let config = dir.path().join("knowledge.toml");
        let f = Self {
            walrus,
            _dir: dir,
            repo,
            bucket_dir,
            bucket,
            config,
            commit,
        };
        f.write_config(&f.bucket);
        f
    }

    fn write_config(&self, bucket: &str) {
        let url = &self.walrus.url;
        std::fs::write(
            &self.config,
            format!(
                r#"[walrus]
enabled = true
network = "mainnet"
aggregators = ["{url}"]
publisher = "{url}"
hedge_after_ms = 50
read_after_write_ms = 1000

[knowledge]
bucket = "{bucket}"

[[knowledge.signers]]
repo = "{REPO}"
identity = "{SIGNER}"
"#
            ),
        )
        .unwrap();
    }

    fn config(&self) -> &str {
        self.config.to_str().unwrap()
    }

    fn publish_args(&self) -> Vec<String> {
        [
            "publish",
            "--repo-path",
            self.repo.to_str().unwrap(),
            "--repo",
            REPO,
            "--visibility",
            "public",
            "--config",
            self.config(),
        ]
        .map(String::from)
        .to_vec()
    }

    fn manifest(&self) -> KnowledgeManifest {
        let key = Layout::default().manifest(REPO, &self.commit);
        KnowledgeManifest::parse(&std::fs::read(self.bucket_dir.join(key)).unwrap()).unwrap()
    }
}

fn deps() -> Deps {
    Deps {
        signer: Some(Arc::new(FakeSigner::default())),
        verifier: Some(Arc::new(FakeVerifier)),
    }
}

async fn run(args: &[String]) -> (Result<bool, KnowledgeError>, String) {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let parsed = parse(&refs).unwrap();
    let mut out = Vec::new();
    let result = execute(&parsed, &deps(), &mut out).await;
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn cli_parses_publish_and_verify() {
    let args = parse(&[
        "publish",
        "--repo",
        REPO,
        "--visibility",
        "public",
        "--config",
        "k.toml",
        "--commit",
        "0123456789abcdef0123456789abcdef01234567",
        "--bucket",
        "s3://b/ns",
        "--epochs",
        "10",
        "--permanent",
        "--output",
        "json",
    ])
    .unwrap();
    let KnowledgeCommand::Publish(p) = args.command else {
        panic!("not publish")
    };
    assert_eq!(p.repo, REPO);
    assert_eq!(p.visibility, VisibilityArg::Public);
    assert_eq!(p.repo_path, Path::new("."));
    assert_eq!(p.epochs, Some(10));
    assert!(p.permanent);
    assert_eq!(p.bucket.as_deref(), Some("s3://b/ns"));
    assert_eq!(p.output, Output::Json);
    assert_eq!(p.cosign, Path::new("cosign"));

    let args = parse(&["verify", "--repo", REPO, "--config", "k.toml"]).unwrap();
    let KnowledgeCommand::Verify(v) = args.command else {
        panic!("not verify")
    };
    assert_eq!(v.commit, None);
    assert_eq!(v.output, Output::Text);

    // Storage can be bought at most 53 epochs ahead.
    assert!(
        parse(&[
            "publish",
            "--repo",
            REPO,
            "--visibility",
            "public",
            "--config",
            "k",
            "--epochs",
            "54"
        ])
        .is_err()
    );
    assert!(
        parse(&[
            "publish",
            "--repo",
            REPO,
            "--visibility",
            "secret",
            "--config",
            "k"
        ])
        .is_err()
    );
}

#[test]
fn cli_visibility_is_required() {
    let err = parse(&["publish", "--repo", REPO, "--config", "k.toml"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    assert!(err.to_string().contains("--visibility"), "{err}");
}

#[tokio::test]
async fn cli_publish_prints_manifest_and_quilt() {
    let f = Fixture::new(true).await;
    let (result, out) = run(&f.publish_args()).await;
    assert!(result.unwrap());
    let m = f.manifest();
    let base = f.bucket.trim_end_matches('/');
    let key = Layout::default().manifest(REPO, &f.commit);
    assert_eq!(
        out,
        format!(
            "repo {REPO}\ncommit {}\nquilt_id {}\nmanifest {base}/{key}\nbundle {base}/{key}.sigstore.json\nentries {}\nend_epoch 42\n",
            f.commit,
            m.quilt_id,
            m.entries.len()
        )
    );
    assert_eq!(f.walrus.puts(), 1);

    let mut json = f.publish_args();
    json.extend([
        "--output".into(),
        "json".into(),
        "--commit".into(),
        f.commit.clone(),
    ]);
    let (result, out) = run(&json).await;
    assert!(result.unwrap());
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["commit"], f.commit.as_str());
    assert_eq!(v["manifest"], format!("{base}/{key}"));
    assert_eq!(v["entries"], 3);
    assert!(v["quilt_id"].as_str().unwrap().len() == 43);
}

/// Review focus 5 through the CLI: nothing reaches the publisher.
#[tokio::test]
async fn cli_publish_refuses_private() {
    let f = Fixture::new(true).await;
    for visibility in ["private", "internal", "unknown"] {
        let mut args = f.publish_args();
        args[6] = visibility.into();
        let (result, _) = run(&args).await;
        assert!(
            matches!(result, Err(KnowledgeError::NotPublic { .. })),
            "{result:?}"
        );
    }
    let unmarked = Fixture::new(false).await;
    let (result, _) = run(&unmarked.publish_args()).await;
    assert!(
        matches!(result, Err(KnowledgeError::NotPublic { .. })),
        "{result:?}"
    );
    assert_eq!(f.walrus.puts() + unmarked.walrus.puts(), 0);
}

#[tokio::test]
async fn cli_publish_needs_a_publisher() {
    let f = Fixture::new(true).await;
    let text = std::fs::read_to_string(&f.config).unwrap();
    let no_publisher: String = text
        .lines()
        .filter(|l| !l.starts_with("publisher"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&f.config, no_publisher).unwrap();
    let (result, _) = run(&f.publish_args()).await;
    let err = result.unwrap_err();
    assert!(err.to_string().contains("no Walrus publisher"), "{err}");
    let mut args = f.publish_args();
    args.extend(["--publisher".into(), f.walrus.url.to_string()]);
    assert!(run(&args).await.0.unwrap());
}

#[tokio::test]
async fn cli_verify_ok() {
    let f = Fixture::new(true).await;
    assert!(run(&f.publish_args()).await.0.unwrap());
    let before = f.walrus.reads();
    let verify = ["verify", "--repo", REPO, "--config", f.config()].map(String::from);
    let (result, out) = run(&verify).await;
    assert!(result.unwrap(), "{out}");
    let m = f.manifest();
    assert_eq!(
        out,
        format!(
            "repo {REPO}\ncommit {}\nquilt_id {}\nsignature ok\nentries {} verified, 0 failed\n",
            f.commit,
            m.quilt_id,
            m.entries.len()
        )
    );
    // Every entry was read from Walrus.
    assert_eq!(f.walrus.reads() - before, m.entries.len());
}

#[tokio::test]
async fn cli_verify_reports_bad_patch() {
    let f = Fixture::new(true).await;
    assert!(run(&f.publish_args()).await.0.unwrap());
    let m = f.manifest();
    let bad = m.entry("docs/a.md").unwrap();
    f.walrus.tamper(&bad.quilt_patch_id.to_string());
    let mut verify = ["verify", "--repo", REPO, "--config", f.config()]
        .map(String::from)
        .to_vec();
    verify.extend(["--commit".into(), f.commit.clone()]);
    let (result, out) = run(&verify).await;
    assert!(!result.unwrap(), "{out}");
    assert!(
        out.contains(&format!(
            "entries {} verified, 1 failed\n",
            m.entries.len() - 1
        )),
        "{out}"
    );
    assert!(
        out.contains("FAILED docs/a.md: docs/a.md: expected sha256"),
        "{out}"
    );

    verify.extend(["--output".into(), "json".into()]);
    let (result, out) = run(&verify).await;
    assert!(!result.unwrap());
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["failures"][0]["path"], "docs/a.md");
}

#[tokio::test]
async fn cli_verify_refuses_tampered_manifest() {
    let f = Fixture::new(true).await;
    assert!(run(&f.publish_args()).await.0.unwrap());
    let path = f
        .bucket_dir
        .join(Layout::default().manifest(REPO, &f.commit));
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replacen("\"bytes\": ", "\"bytes\": 1", 1)).unwrap();
    let verify = ["verify", "--repo", REPO, "--config", f.config()].map(String::from);
    let (result, _) = run(&verify).await;
    assert!(
        matches!(result, Err(KnowledgeError::Signature { .. })),
        "{result:?}"
    );
    // A repository with no pinned signer is refused before any read.
    let other = ["verify", "--repo", "someone/else", "--config", f.config()].map(String::from);
    assert!(matches!(
        run(&other).await.0,
        Err(KnowledgeError::UnknownRepo(_))
    ));
}

#[tokio::test]
async fn cli_bucket_flag_overrides_config() {
    let f = Fixture::new(true).await;
    // The config names a bucket that does not exist; --bucket wins.
    f.write_config("file:///nonexistent/loams-knowledge-cli-test/");
    let mut args = f.publish_args();
    args.extend(["--bucket".into(), f.bucket.clone()]);
    assert!(run(&args).await.0.unwrap());
    assert!(f.bucket_dir.join(Layout::default().latest(REPO)).exists());
    let verify = [
        "verify",
        "--repo",
        REPO,
        "--config",
        f.config(),
        "--bucket",
        &f.bucket,
    ]
    .map(String::from);
    assert!(run(&verify).await.0.unwrap());
}
