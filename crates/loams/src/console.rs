//! The console, served by the engine itself at `/ui/` (design §19 §3).
//!
//! The console ships with each deployment (owner, 2026-10-23; design 78
//! retires the hosted console): the engine image and the Linux packages put
//! the built console (`web/apps/console/dist`, Vite `base: '/ui/'`) in
//! [`DEFAULT_DIR`], and the engine serves it on its own HTTP listeners, the
//! main port and the `tailscale serve` target, next to the API. So the
//! console is always same-origin with the engine it manages: it needs no
//! CORS, and it is reachable exactly where the API is (loopback, or the
//! deployment's tailnet), never publicly (design §69).
//!
//! The files are public, like any web app's shell: they carry no data, and
//! everything the console then asks for goes through the authentication
//! layer like any other call. They are mounted outside that layer.
//!
//! **Its Content-Security-Policy is a header** (owner, 2026-10-23). The
//! build bakes a `<meta>` CSP into its pages for static hosting with no
//! server. A header cannot loosen a meta CSP (a browser enforces both), so
//! the engine takes the policy out of the page it serves, sends it as the
//! `Content-Security-Policy` header instead, and adds to `connect-src` and
//! `frame-src` the agentd hosts Studio may call and frame, so previews render
//! in the engine-served console ([`AGENTD_ORIGINS_ENV`], one validation, `*`
//! refused); every other
//! directive stays the build's. One policy, from one source: the build's.
//! `frame-ancestors`, which a meta CSP cannot set, is `'self'` on every
//! response (the console frames its own plugin sandbox).
//!
//! Single-page routing: a path with no file behind it gets `index.html`; a
//! path that looks like a missing asset stays a 404.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use tower::ServiceExt as _;
use tower_http::services::ServeDir;

/// Where the images and the Linux packages install the console.
pub const DEFAULT_DIR: &str = "/usr/share/loams/console";

/// The path the console is mounted at.
pub const MOUNT: &str = "/ui";

/// The agentd hosts Studio in the engine-served console may call
/// (`loams-agentd serve --remote`), separated by spaces or commas, each an
/// exact origin as in `LOAMS_CORS_ORIGINS`. The same variable the
/// statically hosted console's deploy reads (`web/apps/console/deploy`).
pub const AGENTD_ORIGINS_ENV: &str = "LOAMS_AGENTD_ORIGINS";

/// The build's policy (`web/apps/console/vite.config.ts`, `CSP`), for a
/// page that carries none (a development build): never looser than it.
const DEFAULT_CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; \
     connect-src 'self'; frame-src 'self'; object-src 'none'; base-uri 'none'; \
     form-action 'self'";

/// The pages the build gives a meta CSP (vite.config.ts `cspMeta`).
const PAGES: [&str; 2] = ["index.html", "cordis.html"];

const META_OPEN: &str = "<meta http-equiv=\"Content-Security-Policy\" content=\"";

/// What the console is served with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsoleOptions {
    /// [`AGENTD_ORIGINS_ENV`]: exact origins added to `connect-src` and `frame-src`.
    pub agentd_origins: Vec<String>,
}

impl ConsoleOptions {
    /// Reads [`AGENTD_ORIGINS_ENV`]. An entry that is not an exact origin
    /// (`*`, `null`, a path, credentials) is an error naming the variable.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let raw = var(AGENTD_ORIGINS_ENV).unwrap_or_default();
        let mut agentd_origins = Vec::new();
        for entry in raw
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
        {
            let origin = crate::auth::parse_origin(entry)
                .map_err(|e| format!("{AGENTD_ORIGINS_ENV}: {e}"))?;
            if !agentd_origins.contains(&origin) {
                agentd_origins.push(origin);
            }
        }
        Ok(Self { agentd_origins })
    }

    /// [`Self::from_vars`] on the process environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }
}

/// Where the console is served from: `explicit` (`--console-dir`), else
/// [`DEFAULT_DIR`] when it holds a build. `None` when there is no build
/// there, so a source build without the console still starts.
pub fn resolve(explicit: Option<&Path>) -> Option<PathBuf> {
    let dir = explicit.map_or_else(|| PathBuf::from(DEFAULT_DIR), Path::to_path_buf);
    dir.join("index.html").is_file().then_some(dir)
}

/// A page with its meta CSP taken out, and the policy to send as a header.
#[derive(Debug)]
struct Page {
    html: Vec<u8>,
    csp: HeaderValue,
}

struct Console {
    files: ServeDir,
    index: Page,
    cordis: Option<Page>,
}

/// The console's routes: `/ui/…` from `dir`, and `/` redirecting to `/ui/`.
/// `None` when `dir` holds no readable `index.html`.
pub fn router(dir: &Path, options: &ConsoleOptions) -> Option<Router> {
    let [index, cordis] = PAGES.map(|name| {
        let html = std::fs::read(dir.join(name)).ok()?;
        page(html, &options.agentd_origins)
            .map_err(|err| tracing::warn!(%err, page = name, "the console page is not served"))
            .ok()
    });
    let console = Arc::new(Console {
        files: ServeDir::new(dir).append_index_html_on_directories(false),
        index: index?,
        cordis,
    });
    let mount = Router::new()
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        .nest_service(MOUNT, Router::new().fallback(serve).with_state(console))
        .layer(axum::middleware::map_response(secure));
    Some(mount)
}

/// `html` without its meta CSP, and the header policy: the meta's (else
/// [`DEFAULT_CSP`]) with `agentd` added to `connect-src` and `frame-src`.
fn page(html: Vec<u8>, agentd: &[String]) -> Result<Page, String> {
    let text = String::from_utf8(html).map_err(|_| "the page is not UTF-8".to_owned())?;
    let (html, policy) = match text.find(META_OPEN) {
        Some(start) => {
            let content = start + META_OPEN.len();
            let close = text[content..]
                .find('"')
                .map(|i| content + i)
                .ok_or("an unterminated CSP meta tag")?;
            let end = text[close..]
                .find('>')
                .map(|i| close + i + 1)
                .ok_or("an unterminated CSP meta tag")?;
            let policy = text[content..close]
                .replace("&#39;", "'")
                .replace("&apos;", "'");
            let mut html = String::with_capacity(text.len());
            html.push_str(text[..start].trim_end());
            html.push_str(&text[end..]);
            (html, policy)
        }
        None => (text, DEFAULT_CSP.to_owned()),
    };
    if html.contains(META_OPEN) {
        return Err("more than one CSP meta tag".to_owned());
    }
    let csp = HeaderValue::from_str(&header_policy(&policy, agentd))
        .map_err(|_| "the CSP is not a valid header value".to_owned())?;
    Ok(Page {
        html: html.into_bytes(),
        csp,
    })
}

/// `policy` with `agentd` appended to `connect-src` and `frame-src` (which it
/// gains if it has none, as `'self'` and `agentd`), and `frame-ancestors 'self'` unless
/// it sets its own.
fn header_policy(policy: &str, agentd: &[String]) -> String {
    let mut directives: Vec<String> = policy
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_owned)
        .collect();
    let name = |d: &str| {
        d.split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    // The same origins, validated once, go to `connect-src` (Studio calls agentd) and to
    // `frame-src` (Studio previews render in a frame), owner 2026-10-24.
    for directive in ["connect-src", "frame-src"] {
        match directives.iter_mut().find(|d| name(d) == directive) {
            Some(found) => {
                for origin in agentd {
                    found.push(' ');
                    found.push_str(origin);
                }
            }
            // `connect-src` is always present; `frame-src` is added only when there are
            // origins to allow, so a policy without agentd origins is left as it was.
            None if directive == "connect-src" || !agentd.is_empty() => {
                let mut added = format!("{directive} 'self'");
                for origin in agentd {
                    added.push(' ');
                    added.push_str(origin);
                }
                directives.push(added);
            }
            None => {}
        }
    }
    if !directives.iter().any(|d| name(d) == "frame-ancestors") {
        directives.push("frame-ancestors 'self'".to_owned());
    }
    directives.join("; ")
}

async fn serve(State(console): State<Arc<Console>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    match path.as_str() {
        "" | "/" | "/index.html" => return html(&console.index),
        "/cordis.html" => {
            if let Some(cordis) = &console.cordis {
                return html(cordis);
            }
        }
        _ => {}
    }
    let response = match console.files.clone().oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    if response.status() == StatusCode::NOT_FOUND && !looks_like_asset(&path) {
        // A client-side route: the app shell.
        return html(&console.index);
    }
    if response.status() == StatusCode::NOT_FOUND {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("not found: {path}\n"),
        )
            .into_response();
    }
    response
}

fn html(page: &Page) -> Response {
    let mut response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        page.html.clone(),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, page.csp.clone());
    response
}

/// `nosniff`, `no-referrer`, and `frame-ancestors 'self'` where a response
/// has no policy of its own (the pages carry the full one).
async fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert(HeaderValue::from_static("frame-ancestors 'self'"));
    response
}

/// Whether a path names a file the build ships, rather than a client-side route.
fn looks_like_asset(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.') && !last.contains(' '))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use std::collections::HashMap;

    const BUILD_INDEX: &str = "<!doctype html>\n<html>\n  <head>\n    <meta charset=\"utf-8\" />\n    \
        <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'self'; script-src 'self' \
        'wasm-unsafe-eval'; connect-src 'self'; frame-src 'self'; object-src 'none'\" />\n    \
        <title>Loams</title>\n  </head>\n</html>\n";

    fn build() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("index.html"), BUILD_INDEX).unwrap();
        std::fs::write(dir.path().join("cordis.html"), BUILD_INDEX).unwrap();
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "console.log(1)").unwrap();
        dir
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    async fn get(router: &Router, path: &str) -> Response {
        router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn csp(response: &Response) -> &str {
        response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
    }

    #[test]
    fn no_build_no_mount() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(router(dir.path(), &ConsoleOptions::default()).is_none());
        assert!(resolve(Some(dir.path())).is_none());
        assert!(resolve(Some(build().path())).is_some());
    }

    #[test]
    fn agentd_origins_are_exact_origins() {
        assert_eq!(
            ConsoleOptions::from_vars(vars(&[])).unwrap(),
            ConsoleOptions::default()
        );
        let options = ConsoleOptions::from_vars(vars(&[(
            AGENTD_ORIGINS_ENV,
            "https://Agentd.Example.ts.net/ https://b.example.ts.net:8443,https://agentd.example.ts.net",
        )]))
        .unwrap();
        assert_eq!(
            options.agentd_origins,
            [
                "https://agentd.example.ts.net",
                "https://b.example.ts.net:8443"
            ]
        );
        for bad in [
            "*",
            "null",
            "https://*.ts.net",
            "https://a.ts.net/path",
            "https://user@a.ts.net",
            "a.ts.net",
        ] {
            let err = ConsoleOptions::from_vars(vars(&[(AGENTD_ORIGINS_ENV, bad)])).unwrap_err();
            assert!(err.starts_with("LOAMS_AGENTD_ORIGINS: "), "{bad}: {err}");
        }
    }

    #[tokio::test]
    async fn the_page_has_no_meta_csp_and_the_header_has_the_builds_policy() {
        let dir = build();
        let router = router(dir.path(), &ConsoleOptions::default()).unwrap();
        for path in [
            "/ui/",
            "/ui",
            "/ui/index.html",
            "/ui/cordis.html",
            "/ui/projects/x",
        ] {
            let page = get(&router, path).await;
            assert_eq!(page.status(), StatusCode::OK, "{path}");
            assert_eq!(
                csp(&page),
                "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; \
                 frame-src 'self'; object-src 'none'; frame-ancestors 'self'",
                "{path}"
            );
            assert_eq!(page.headers()["x-content-type-options"], "nosniff");
            let html = body(page).await;
            assert!(!html.contains("Content-Security-Policy"), "{path}: {html}");
            assert!(
                html.contains("<meta charset=\"utf-8\" />\n    <title>"),
                "{path}: {html}"
            );
        }
    }

    #[tokio::test]
    async fn agentd_origins_join_connect_src_and_frame_src() {
        let dir = build();
        let options = ConsoleOptions {
            agentd_origins: vec![
                "https://agentd.example.ts.net".into(),
                "https://b.example.ts.net:8443".into(),
            ],
        };
        let router = router(dir.path(), &options).unwrap();
        let page = get(&router, "/ui/").await;
        assert_eq!(
            csp(&page),
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self' \
             https://agentd.example.ts.net https://b.example.ts.net:8443; frame-src 'self' \
             https://agentd.example.ts.net https://b.example.ts.net:8443; \
             object-src 'none'; frame-ancestors 'self'"
        );
    }

    #[tokio::test]
    async fn a_page_without_a_meta_csp_gets_the_builds_policy() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!doctype html><title>dev</title>",
        )
        .unwrap();
        let router = router(dir.path(), &ConsoleOptions::default()).unwrap();
        let page = get(&router, "/ui/").await;
        assert_eq!(
            csp(&page),
            format!("{}; frame-ancestors 'self'", DEFAULT_CSP.replace("  ", " "))
        );
        // No cordis.html in this build: a 404, not the shell.
        assert_eq!(
            get(&router, "/ui/cordis.html").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn assets_config_and_routes() {
        let dir = build();
        let router = router(dir.path(), &ConsoleOptions::default()).unwrap();
        let asset = get(&router, "/ui/assets/app.js").await;
        assert_eq!(asset.status(), StatusCode::OK);
        // Every response may be framed by the console only (its plugin sandbox).
        assert_eq!(csp(&asset), "frame-ancestors 'self'");
        let config = get(&router, "/ui/config.json").await;
        assert_eq!(config.status(), StatusCode::OK);
        assert_eq!(body(config).await, "{}");
        let missing = get(&router, "/ui/assets/gone.js").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let redirect = get(&router, "/").await;
        assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(redirect.headers()[header::LOCATION], "/ui/");
    }

    #[test]
    fn a_route_is_not_mistaken_for_a_missing_asset() {
        for route in ["/", "/settings", "/projects/loams"] {
            assert!(!looks_like_asset(route), "{route}");
        }
        for asset in ["/assets/app.js", "/favicon.svg", "/cordis.html"] {
            assert!(looks_like_asset(asset), "{asset}");
        }
    }
}
