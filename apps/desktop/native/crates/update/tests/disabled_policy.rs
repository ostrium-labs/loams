//! This integration test links the production library (without cfg(test)).
use loams_desktop_update::{Updater, desktop_auto_update_enabled, fetch_latest, updates_enabled};

#[tokio::test]
async fn shipping_builds_cannot_fetch_or_enable_self_updates() {
    assert!(!updates_enabled());
    assert!(!desktop_auto_update_enabled());
    // A refused loopback endpoint makes accidental network fallback visible.
    let error = fetch_latest("http://127.0.0.1:1").await.unwrap_err();
    assert!(error.to_string().contains("self-updates are disabled"));
    let error =
        loams_desktop_update::apply_headless(std::path::Path::new("/unused"), "1.0.0").unwrap_err();
    assert!(error.to_string().contains("self-updates are disabled"));
    let updater = Updater::spawn_desktop("http://127.0.0.1:1".into());
    assert!(updater.watch().borrow().checked_at.is_none());
    updater.shutdown().await;
}
