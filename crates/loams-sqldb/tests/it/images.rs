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
    assert_eq!(tidb.image, "docker.io/pingcap/tidb");
    assert!(tidb.tag.starts_with("v8.5."), "{}", tidb.tag);
    // Every TiDB-ecosystem image moves together.
    for (name, pin) in images.all() {
        assert_eq!(pin.tag, tidb.tag, "{name}");
    }
    // The advertised version names the pinned release.
    assert_eq!(
        loams_sqldb::render::SERVER_VERSION,
        format!("8.0.11-TiDB-{}-Loams", tidb.tag)
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
