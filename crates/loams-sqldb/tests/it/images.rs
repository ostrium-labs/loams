use loams_sqldb::images::{ImageError, Images};

fn is_sha256_digest(d: &str) -> bool {
    d.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[test]
fn images_are_pinned_by_digest() {
    let images = Images::load().expect("release/sqldb-images.toml parses");
    for (name, pin) in images.all() {
        assert!(is_sha256_digest(&pin.digest), "{name}: {}", pin.digest);
        let reference = pin.reference();
        assert_eq!(reference, format!("{}@{}", pin.image, pin.digest), "{name}");
        assert!(
            !pin.image.contains('@'),
            "{name}: digest belongs in `digest`"
        );
        // The image name carries no tag: the runtime pulls by digest only.
        let last = pin.image.rsplit('/').next().unwrap_or_default();
        assert!(!last.contains(':'), "{name}: tag in image name");
    }

    // A tag-only entry is refused.
    let tag_only = r#"
        [tidb]
        image = "docker.io/pingcap/tidb"
        tag = "v8.5.8"
        digest = "v8.5.8"
    "#;
    assert!(matches!(
        Images::parse(&with_others(tag_only)),
        Err(ImageError::NotPinned { .. })
    ));
    let tagged_name = r#"
        [tidb]
        image = "docker.io/pingcap/tidb:v8.5.8"
        tag = "v8.5.8"
        digest = "sha256:df168c764bf2dfdb166dc37a5c3b0e210d29d5f3ab2d33317fd0fdf7b32037f5"
    "#;
    assert!(matches!(
        Images::parse(&with_others(tagged_name)),
        Err(ImageError::NotPinned { .. })
    ));
}

#[test]
fn tidb_image_is_v8_5() {
    let images = Images::load().expect("pins");
    let tidb = images.tidb();
    // TiDB is the ostrium-labs/tidb patch queue on an upstream release
    // (§47 §5.4, R5.3): `<release>-loams.<n>`.
    assert_eq!(tidb.image, "ghcr.io/ostrium-labs/tidb");
    let (release, patch) = tidb
        .tag
        .split_once("-loams.")
        .expect("a v8.5.x-loams.N tag");
    assert!(release.starts_with("v8.5."), "{}", tidb.tag);
    assert!(patch.parse::<u32>().is_ok(), "{}", tidb.tag);
    // Every TiDB-ecosystem image moves together, on the same release.
    for (name, pin) in images.all() {
        if name != "tidb" {
            assert_eq!(pin.tag, release, "{name}");
            assert!(pin.image.starts_with("docker.io/pingcap/"), "{name}");
        }
    }
    // The advertised version names the upstream release.
    assert_eq!(
        loams_sqldb::render::SERVER_VERSION,
        format!("8.0.11-TiDB-{release}-Loams")
    );
}

/// The four images other than `tidb`, copied from the real file, so a test
/// can vary one entry.
fn with_others(tidb: &str) -> String {
    let real = include_str!("../../../../release/sqldb-images.toml");
    let rest: String = real
        .split("\n[")
        .skip(1)
        .filter(|s| !s.starts_with("tidb]"))
        .map(|s| format!("\n[{s}"))
        .collect();
    format!("{tidb}\n{rest}")
}
