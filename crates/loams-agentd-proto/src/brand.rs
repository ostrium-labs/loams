//! Loams identity strings, in one place.
//!
//! Moved from the fork's `loams-desktop-brand` crate (Apache-2.0, written by
//! The Loams Authors), which the Loams Authors place here under this crate's
//! MIT licence (plan DD1 ruling T0-10a).

/// The product, as shown to users.
pub const PRODUCT_NAME: &str = "Loams Desktop";
/// The daemon's executable and CLI command name.
pub const BINARY_NAME: &str = "loams-agentd";
/// Desktop-owned runtime environment variables.
pub const ENV_PREFIX: &str = "LOAMS_DESKTOP_";
/// Unix data directory beneath the user's home (no upstream migration).
pub const UNIX_DATA_DIR: &str = ".loams-desktop";
/// The chat that drives the platform agents (owner ruling, 2026-10-02).
pub const BOT_NAME: &str = "Loams Bot";
/// The loop Loams Bot drives (owner ruling, 2026-10-02).
pub const FACTORY_NAME: &str = "Loams Software Factory";
/// Reverse-DNS application id (Linux app id, macOS bundle id, Windows AUMID).
pub const APP_ID: &str = "dev.loams.desktop";
/// The public OAuth client id registered as an Authentik application
/// (design 37 section 6.5).
pub const OIDC_CLIENT_ID: &str = "loams-desktop";
/// The OS keychain service name for stored credentials.
pub const KEYRING_SERVICE: &str = "dev.loams.desktop";
/// The `loams.dev` custom URL scheme (navigation only, design 37 section 6.7).
pub const URL_SCHEME: &str = "loams";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_identity_is_isolated_from_core_and_upstream() {
        assert_eq!(PRODUCT_NAME, "Loams Desktop");
        assert_eq!(BINARY_NAME, "loams-agentd");
        assert_eq!(ENV_PREFIX, "LOAMS_DESKTOP_");
        assert_eq!(UNIX_DATA_DIR, ".loams-desktop");
        assert_eq!(APP_ID, KEYRING_SERVICE);
        assert_eq!(APP_ID, "dev.loams.desktop");
        assert_eq!(URL_SCHEME, "loams");
    }
}
