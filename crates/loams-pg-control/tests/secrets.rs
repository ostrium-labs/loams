//! `FileSecretStore` (PG2 Task 6): single-node mode's age-encrypted secret
//! file and its key file.

use std::path::Path;

use loams_pg_control::ids::ProjectId;
use loams_pg_control::secrets::{
    FileSecretStore, KeySource, Secret, SecretError, SecretRef, SecretStore,
};

fn dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("secrets-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("a temporary directory")
}

fn open(dir: &Path) -> FileSecretStore {
    FileSecretStore::open(
        dir.join("secrets.age"),
        &KeySource::File(dir.join("key/identity")),
    )
    .expect("the store")
}

fn new_ref() -> SecretRef {
    SecretRef::new_role(&ProjectId::new())
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn file_store_round_trips_and_survives_reopen() {
    let d = dir();
    let store = open(d.path());
    let (a, b) = (new_ref(), new_ref());
    let password = b"correct-horse-battery-staple-0123456789abcdef".to_vec();
    store
        .put(&a, Secret::new(password.clone()))
        .await
        .expect("put a");
    store
        .put(&b, Secret::new(b"other".to_vec()))
        .await
        .expect("put b");
    assert_eq!(store.get(&a).await.expect("get a").expose(), &password);

    // A second store on the same file and key reads what the first wrote.
    let again = open(d.path());
    assert_eq!(again.get(&a).await.expect("reopened").expose(), &password);
    again.delete(&a).await.expect("delete");
    again
        .delete(&a)
        .await
        .expect("deleting an absent secret is fine");
    assert!(matches!(
        store.get(&a).await,
        Err(SecretError::NotFound(r)) if r == a.as_str()
    ));
    assert_eq!(store.get(&b).await.expect("b stays").expose(), b"other");
    assert_eq!(store.list().await.expect("list"), vec![b.clone()]);

    // The file holds ciphertext only, and neither file is readable by others.
    let on_disk = std::fs::read(d.path().join("secrets.age")).expect("the file");
    assert!(!on_disk.windows(5).any(|w| w == b"other"));
    assert!(on_disk.starts_with(b"age-encryption.org/v1"));
    #[cfg(unix)]
    {
        assert_eq!(mode(&d.path().join("secrets.age")), 0o600);
        assert_eq!(mode(&d.path().join("key/identity")), 0o600);
    }
    let key = std::fs::read_to_string(d.path().join("key/identity")).expect("the key");
    assert!(key.starts_with("AGE-SECRET-KEY-1"));
    assert!(!format!("{store:?}").contains(key.trim()));
}

#[tokio::test]
async fn a_missing_file_is_empty() {
    let d = dir();
    let store = open(d.path());
    let r = new_ref();
    assert!(matches!(store.get(&r).await, Err(SecretError::NotFound(_))));
    store.delete(&r).await.expect("nothing to delete");
    assert!(
        !d.path().join("secrets.age").exists(),
        "a delete of nothing writes nothing"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_key_file_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let d = dir();
    drop(open(d.path()));
    let key = d.path().join("key/identity");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let e = FileSecretStore::open(d.path().join("secrets.age"), &KeySource::File(key))
        .expect_err("mode 0644");
    assert!(matches!(e, SecretError::Config(_)), "{e}");
}

#[tokio::test]
async fn another_key_does_not_decrypt_the_file() {
    let d = dir();
    let store = open(d.path());
    let r = new_ref();
    store
        .put(&r, Secret::new(b"s3cret".to_vec()))
        .await
        .expect("put");
    let other_key = dir();
    let thief = FileSecretStore::open(
        d.path().join("secrets.age"),
        &KeySource::File(other_key.path().join("identity")),
    )
    .expect("a store with another key");
    let e = thief.get(&r).await.expect_err("another key");
    assert!(matches!(e, SecretError::Corrupt(_)), "{e}");
    assert!(!e.to_string().contains("s3cret"));
}

#[tokio::test]
async fn a_key_that_does_not_parse_is_refused() {
    let d = dir();
    let key = d.path().join("identity");
    {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&key)
            .and_then(|mut f| f.write_all(b"not a key\n"))
            .expect("a bad key");
    }
    let e = FileSecretStore::open(d.path().join("secrets.age"), &KeySource::File(key))
        .expect_err("a bad key");
    assert!(matches!(e, SecretError::Config(_)), "{e}");
}

#[tokio::test]
async fn concurrent_puts_lose_nothing() {
    let d = dir();
    let store = open(d.path());
    let refs: Vec<SecretRef> = (0..16).map(|_| new_ref()).collect();
    let mut tasks = Vec::new();
    for (i, r) in refs.iter().enumerate() {
        let (store, r) = (store.clone(), r.clone());
        tasks.push(tokio::spawn(async move {
            store
                .put(&r, Secret::new(vec![u8::try_from(i).expect("small"); 4]))
                .await
        }));
    }
    for t in tasks {
        t.await.expect("join").expect("put");
    }
    for (i, r) in refs.iter().enumerate() {
        assert_eq!(
            store.get(r).await.expect("get").expose(),
            &vec![u8::try_from(i).expect("small"); 4]
        );
    }
}
