//! The built console, served at `/ui/`.
//!
//! The console is a Vite app that builds to `web/apps/console/dist` with
//! `base: '/ui/'` (A§19 A§3). In development its own dev server proxies the
//! API paths here; serving the build from the same listener means a phone or a
//! second browser tab can open the console on the one address the mock is
//! already on.
//!
//! Single-page app routing: a path that is not a file falls back to
//! `index.html`, so a deep link like `/ui/settings` boots the app rather than
//! 404ing.

use std::path::{Path, PathBuf};

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse as _;
use tower_http::services::ServeDir;

/// Where the console's build lands, relative to the workspace root.
const BUILD: &str = "web/apps/console/dist";

/// Mounts the console at `/ui/`, or returns `None` when it has not been built,
/// so the mock still starts for someone who only wants the RPCs.
pub(crate) fn router(dir: Option<&Path>) -> Option<Router> {
    let dir = match dir.map(Path::to_path_buf).or_else(find_build) {
        Some(dir) if dir.join("index.html").is_file() => dir,
        _ => return None,
    };
    let index = dir.join("index.html");
    tracing::info!(path = %dir.display(), "console UI mounted at /ui/");
    Some(Router::new().nest_service("/ui", ServeDir::new(&dir).fallback(app_shell(index))))
}

/// The single-page app fallback: a path with no file behind it is a
/// client-side route, so it gets `index.html` with a 200. A path that looks
/// like a missing *asset* stays a 404, or a mistyped bundle path would come
/// back as a parse error in the app rather than a clear 404.
fn app_shell(index: PathBuf) -> Router {
    Router::new().fallback(move |uri: axum::http::Uri| async move {
        let path = uri.path();
        if looks_like_asset(path) {
            return (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                format!("not found: {path}\n"),
            )
                .into_response();
        }
        match tokio::fs::read(&index).await {
            Ok(body) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                body,
            )
                .into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                format!("console index.html unreadable: {err}\n"),
            )
                .into_response(),
        }
    })
}

/// Whether a path names a file the build ships, rather than a client-side route.
fn looks_like_asset(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.') && !last.contains(' '))
}

/// Finds `web/apps/console/dist` by walking up from the working directory, so
/// `cargo run -p loams-apps-mock` finds it without a flag.
fn find_build() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(BUILD);
        if candidate.join("index.html").is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The hint to print when there is no build to serve.
pub(crate) const NO_BUILD_HINT: &str = "console UI not mounted: build it with `pnpm --filter @loams/console build` \
     (or pass --ui-dir) to serve it at /ui/";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_an_index_is_not_served() {
        let dir = std::env::temp_dir().join("loams-ui-missing-index");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(router(Some(&dir)).is_none(), "no index.html, no mount");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_explicit_dir_without_a_build_is_not_served() {
        let dir = std::env::temp_dir().join("loams-ui-never-built");
        assert!(router(Some(&dir)).is_none());
    }

    #[test]
    fn a_route_is_not_mistaken_for_a_missing_asset() {
        // Client-side routes have no extension and get the app shell.
        for route in ["/", "/settings", "/projects/loams", "/settings/network"] {
            assert!(!looks_like_asset(route), "{route} is a route");
        }
        // Build output does, and must stay a 404 when absent.
        for asset in [
            "/assets/app.js",
            "/assets/app-Bq7x2.css",
            "/cordis.html",
            "/favicon.svg",
            "/.well-known/x.json",
        ] {
            assert!(looks_like_asset(asset), "{asset} is an asset");
        }
    }

    #[test]
    fn a_build_with_an_index_is_served() {
        let dir = std::env::temp_dir().join("loams-ui-built");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<!doctype html><title>Loams</title>",
        )
        .unwrap();
        std::fs::write(dir.join("assets/app.js"), "console.log('loams')").unwrap();
        assert!(router(Some(&dir)).is_some(), "an index.html, a mount");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
