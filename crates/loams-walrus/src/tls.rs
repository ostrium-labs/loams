//! The process's rustls provider, for https aggregators and publishers.

/// Installs ring's provider when the process has none (the `loams` binary
/// installs it at start; tests and embedders may not).
pub(crate) fn ensure_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Losing a race with another installer is fine: one is installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}
