//! The browser provider parity suite (BR-b, D2120–D2129).
//!
//! The same scenarios run against every selected provider:
//!
//! - **`local`**: the pinned Obscura binary, launched by the suite through
//!   the Desktop's own [`loams_sidebar_browser::ObscuraEngine`].
//! - **`service`**: the same engine as the `browser` service, the pinned
//!   Obscura image in compose or k8s, reached over its DevTools WebSocket with
//!   a bearer token.
//!
//! The owner's requirement is that the built-in browser behaves the same
//! locally and in the cloud. So nothing here is per-provider except where a
//! capability genuinely differs, and then the scenario reports `n/a` with the
//! reason instead of passing.
//!
//! Every page comes from [`fixture::Fixture`], an HTTP server in this process
//! on 127.0.0.1. No scenario touches the public internet. Selection and
//! configuration are by environment variable (see [`provider`]); with nothing
//! selected every scenario prints *skipped* and passes, so the workspace test
//! run is unaffected. CI selects one provider per job, sets
//! `LOAMS_BROWSER_PARITY_REQUIRE=1`, and builds the parity matrix from
//! `LOAMS_BROWSER_PARITY_REPORT` (`.github/workflows/browser-parity.yml`,
//! `docs/browser-provider-parity.md`).
//!
//! This is the **engine-level** layer: CDP plus Loams' own policy pieces (the
//! navigation allow list, the cookie ledger, the boundary audit, the
//! recorder). The contract-level layer, which drives
//! `loams_web_bridge::BrowserProvider`, follows once BR-a's `ObscuraProvider`
//! lands (`.he0-shared/browser-notes.md`).

mod fixture;
mod page;
mod png;
mod provider;
mod report;

use std::time::{Duration, Instant};

use base64::Engine as _;
use loams_sidebar_browser::{
    Container, CookieLedger, EmbeddedOrigin, FrameDisposition, FrameFormat, NavigationAllowlist,
    PrivateNetworkDecision, ProfileKey, RecorderConfig, ScopedSessionCookie, VideoRecorder,
    assert_cookies_exclude, audit_profile, cookies_from_cdp, decide_origin, decode_frame,
    png_dimensions, resolve_ffmpeg,
};
use serde_json::{Value, json};

use fixture::{DOWNLOAD_BYTES, EXTRACT_SENTINEL, Fixture};
use page::{Verdict, click, eval, navigate, run_intercepted, type_text, wait_until};
use provider::{Kind, Provider, close_on, open_on};
use report::{Failure, Outcome, ensure, run};

/// How long a fixture page may take to load.
const LOAD: Duration = Duration::from_secs(20);

fn origin(fixture: &Fixture) -> EmbeddedOrigin {
    EmbeddedOrigin::parse(&fixture.origin()).expect("a fixture origin")
}

async fn connect(kind: Kind, fixture: &Fixture) -> Result<Provider, Failure> {
    Provider::connect(kind, &origin(fixture))
        .await
        .map_err(Failure::Unconfigured)
}

/// A value unlikely to collide with anything a page or engine would produce.
fn nonce(label: &str) -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default(),
    );
    hasher.write_u32(std::process::id());
    format!("{label}{:016x}", hasher.finish())
}

fn text_contains(value: &Value, needle: &str) -> bool {
    value.as_str().is_some_and(|text| text.contains(needle))
}

// ------------------------------------------------------------------ navigate

async fn navigate_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    navigate(&mut provider.client, &page, &fixture.url("/"), LOAD).await?;
    let title = eval(&mut provider.client, &page, "document.title").await?;
    ensure(title == json!("Parity fixture"), || {
        format!("the title is {title}, not \"Parity fixture\"")
    })?;
    navigate(
        &mut provider.client,
        &page,
        &fixture.url("/inline.txt"),
        LOAD,
    )
    .await?;
    let href = eval(&mut provider.client, &page, "location.href").await?;
    ensure(href == json!(fixture.url("/inline.txt")), || {
        format!("location.href is {href} after the second navigation")
    })?;
    ensure(!fixture.seen_path("/inline.txt").is_empty(), || {
        "the fixture never saw the second navigation's request".to_string()
    })?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok("two navigations, title and URL as expected".to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigate_to_a_fixture_page() {
    run("navigate", navigate_scenario).await;
}

// ------------------------------------------------------ accessibility snapshot

/// The `(role, name)` pairs the fixture's index page must expose.
const EXPECTED_AX: &[(&str, &str)] = &[
    ("heading", "Fixture login"),
    ("link", "Docs"),
    ("textbox", "Email"),
    ("textbox", "Note"),
    ("button", "Sign in"),
];

/// A DOM-derived accessibility snapshot, for an engine with no
/// `Accessibility` domain. Implicit roles and label-derived names only: the
/// subset the fixture uses, not a general accessible-name computation.
const DOM_AX: &str = r#"(() => {
  const out = [];
  const name = (el) => (el.getAttribute('aria-label') || '').trim()
    || (el.id && document.querySelector(`label[for="${el.id}"]`)?.textContent.trim())
    || (el.textContent || '').trim();
  for (const el of document.querySelectorAll('h1,h2,h3,h4,h5,h6,a[href],button,input,textarea,select')) {
    const tag = el.tagName.toLowerCase();
    let role = el.getAttribute('role');
    if (!role) {
      if (/^h[1-6]$/.test(tag)) role = 'heading';
      else if (tag === 'a') role = 'link';
      else if (tag === 'button') role = 'button';
      else if (tag === 'textarea') role = 'textbox';
      else if (tag === 'select') role = 'combobox';
      else if (tag === 'input') {
        const type = (el.getAttribute('type') || 'text').toLowerCase();
        role = ({ checkbox: 'checkbox', radio: 'radio', submit: 'button', button: 'button' })[type] || 'textbox';
      }
    }
    out.push([role, name(el)]);
  }
  return out;
})()"#;

fn ax_pairs_from_tree(tree: &Value) -> Vec<(String, String)> {
    tree.get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| {
                    let role = node.get("role")?.get("value")?.as_str()?.to_string();
                    let name = node
                        .get("name")
                        .and_then(|name| name.get("value"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    Some((role, name))
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn snapshot_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    navigate(&mut provider.client, &page, &fixture.url("/"), LOAD).await?;
    let (pairs, via) = match provider
        .client
        .call(
            "Accessibility.getFullAXTree",
            json!({}),
            Some(&page.session_id),
        )
        .await
    {
        Ok(tree) => (ax_pairs_from_tree(&tree), "Accessibility.getFullAXTree"),
        Err(error) => {
            let value = eval(&mut provider.client, &page, DOM_AX).await?;
            let pairs = value
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| {
                            Some((
                                row.get(0)?.as_str()?.to_string(),
                                row.get(1)?.as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            eprintln!("Accessibility.getFullAXTree unavailable ({error}); using the DOM snapshot");
            (pairs, "DOM-derived snapshot (no Accessibility domain)")
        }
    };
    let missing: Vec<String> = EXPECTED_AX
        .iter()
        .filter(|(role, name)| {
            !pairs
                .iter()
                .any(|(found_role, found_name)| found_role == role && found_name == name)
        })
        .map(|(role, name)| format!("{role} {name:?}"))
        .collect();
    ensure(missing.is_empty(), || {
        format!("via {via}: missing {missing:?}; saw {pairs:?}")
    })?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok(format!("{} nodes via {via}", pairs.len()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accessibility_snapshot() {
    run("accessibility_snapshot", snapshot_scenario).await;
}

// ------------------------------------------------------------ click and type

async fn forms_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    navigate(&mut provider.client, &page, &fixture.url("/"), LOAD).await?;
    let client = &mut provider.client;
    click(client, &page, "#email").await?;
    type_text(client, &page, "agent@example.test").await?;
    click(client, &page, "#note").await?;
    type_text(client, &page, "parity note").await?;
    let values = eval(
        client,
        &page,
        "[document.querySelector('#email').value, document.querySelector('#note').value]",
    )
    .await?;
    ensure(
        values == json!(["agent@example.test", "parity note"]),
        || format!("after clicking and typing, the inputs hold {values}"),
    )?;
    click(client, &page, "#submit").await?;
    wait_until(
        client,
        &page,
        LOAD,
        |value| text_contains(value, "Thanks, agent@example.test / parity note"),
        "document.querySelector('#result')?.textContent ?? null",
    )
    .await?;
    let posts = fixture.seen_path("/submit");
    ensure(
        posts.iter().any(|seen| {
            seen.method == "POST"
                && seen.body.contains("email=agent%40example.test")
                && seen.body.contains("note=parity")
        }),
        || format!("the fixture did not receive the form as a POST: {posts:?}"),
    )?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok("clicked two inputs, typed, submitted; server saw the POST".to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn click_and_type_a_form() {
    run("click_type_forms", forms_scenario).await;
}

// ---------------------------------------------------------------- screenshot

async fn screenshot_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    navigate(&mut provider.client, &page, &fixture.url("/"), LOAD).await?;
    let shot = provider
        .client
        .call(
            "Page.captureScreenshot",
            json!({ "format": "png" }),
            Some(&page.session_id),
        )
        .await
        .map_err(|error| format!("Page.captureScreenshot: {error}"))?;
    let data = shot
        .get("data")
        .and_then(Value::as_str)
        .ok_or("Page.captureScreenshot returned no data")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("the screenshot is not base64: {error}"))?;
    let (width, height) =
        png_dimensions(&bytes).map_err(|error| format!("not a PNG header: {error}"))?;
    ensure(width >= 320 && height >= 160, || {
        format!("{width}x{height} is smaller than the fixture's coloured block")
    })?;
    let image = png::decode(&bytes)?;
    ensure((image.width, image.height) == (width, height), || {
        format!(
            "IHDR says {width}x{height}, the pixels say {}x{}",
            image.width, image.height
        )
    })?;
    let share = image.non_background_share();
    let red = image.reddish_pixels();
    ensure(share > 0.01, || {
        format!("the screenshot is blank: {share:.4} of pixels differ from the background")
    })?;
    ensure(red >= 1000, || {
        format!("the fixture's red block is missing: {red} reddish pixels")
    })?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok(format!(
        "{width}x{height} PNG, {:.1}% non-background, {red} red pixels",
        share * 100.0
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screenshot_has_dimensions_and_is_not_blank() {
    run("screenshot", screenshot_scenario).await;
}

// -------------------------------------------------------- content extraction

async fn extract_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    navigate(&mut provider.client, &page, &fixture.url("/"), LOAD).await?;
    let text = eval(&mut provider.client, &page, "document.body.innerText").await?;
    let text = text.as_str().unwrap_or_default().to_string();
    ensure(text.contains(EXTRACT_SENTINEL), || {
        format!("innerText does not contain the sentinel: {text:?}")
    })?;
    ensure(text.contains("Fixture login"), || {
        format!("innerText lost the heading: {text:?}")
    })?;
    let html = eval(
        &mut provider.client,
        &page,
        "document.documentElement.outerHTML",
    )
    .await?;
    ensure(
        html.as_str()
            .is_some_and(|html| html.contains("id=\"login\"")),
        || "outerHTML does not contain the form".to_string(),
    )?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok(format!("innerText {} chars", text.len()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_extraction() {
    run("content_extraction", extract_scenario).await;
}

// ----------------------------------------------------------------- downloads
//
// The download policy is set through `Browser.setDownloadBehavior`, the CDP
// call a browser honours for both "deny" and "allow into this directory"
// (D2126). A download is detected by what a browser that downloads does: a
// `Browser.downloadWillBegin` / `Page.downloadWillBegin` event, or a file
// with the attachment's bytes in the download directory.
//
// Read from Obscura's source at v0.2.3 (2026-10-28): it acks
// `setDownloadBehavior` without acting on it, because the engine never writes
// a download to disk, and it emits no download events. Its `Fetch` domain
// pauses at the request stage only. So "blocked" holds by construction, and
// "allowed" cannot be honoured. Both scenarios say exactly that.

const FETCH_RESULT: &str = "window.__r ?? null";

fn fetch_into_r(url: &str) -> String {
    format!(
        "window.__r = null; fetch({}).then(r => r.arrayBuffer()).then(b => {{ window.__r = {{ ok: true, len: b.byteLength }}; }}).catch(e => {{ window.__r = {{ ok: false, error: String(e) }}; }}); true",
        json!(url)
    )
}

/// Files under `dir` whose bytes contain the fixture attachment.
fn downloaded_files(dir: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read(&path).is_ok_and(|bytes| {
                bytes
                    .windows(DOWNLOAD_BYTES.len())
                    .any(|window| window == DOWNLOAD_BYTES)
            }) {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

/// Set the policy, navigate to the attachment, and report the download events
/// seen within a few seconds.
async fn attempt_download(
    provider: &mut Provider,
    page: &provider::PageHandle,
    fixture: &Fixture,
    behavior: Value,
) -> Result<Vec<String>, Failure> {
    provider
        .client
        .call("Browser.setDownloadBehavior", behavior, None)
        .await
        .map_err(|error| format!("Browser.setDownloadBehavior: {error}"))?;
    navigate(&mut provider.client, page, &fixture.url("/"), LOAD).await?;
    // The attachment as a top-level navigation, which is how a click on a
    // download link reaches a browser. Its result is not the point: some
    // engines answer with an error for a download, some with nothing.
    let _ = provider
        .client
        .call(
            "Page.navigate",
            json!({ "url": fixture.url("/download.bin") }),
            Some(&page.session_id),
        )
        .await;
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(event) =
            page::next_event(&mut provider.client, Duration::from_millis(250)).await?
            && event.method.to_ascii_lowercase().contains("download")
        {
            events.push(event.method);
        }
    }
    ensure(!fixture.seen_path("/download.bin").is_empty(), || {
        "the engine never requested the attachment, so the scenario proved nothing".to_string()
    })?;
    Ok(events)
}

async fn downloads_blocked_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    let events = attempt_download(
        &mut provider,
        &page,
        &fixture,
        json!({ "behavior": "deny" }),
    )
    .await?;
    let begun: Vec<&String> = events
        .iter()
        .filter(|method| method.ends_with("downloadWillBegin"))
        .collect();
    ensure(begun.is_empty(), || {
        format!("a download began under the deny policy: {events:?}")
    })?;
    // On local the suite owns the engine's filesystem view; on service the CI
    // job checks the container with `docker diff` after the run.
    let written = downloaded_files(&provider.profile_dir);
    ensure(written.is_empty(), || {
        format!("the attachment was written to disk under deny: {written:?}")
    })?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok(format!(
        "deny accepted; no download event, no file{}",
        if kind == Kind::Service {
            " (container filesystem checked by CI)"
        } else {
            ""
        }
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_blocked_by_policy() {
    run("downloads_blocked", downloads_blocked_scenario).await;
}

async fn downloads_allowed_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    // On local the directory is the suite's; on service it is a path inside
    // the container, which only the events can speak for.
    let directory = match kind {
        Kind::Local => {
            let directory = provider.profile_dir.join("downloads");
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            directory.display().to_string()
        }
        Kind::Service => "/tmp/loams-parity-downloads".to_string(),
    };
    let events = attempt_download(
        &mut provider,
        &page,
        &fixture,
        json!({ "behavior": "allow", "downloadPath": directory, "eventsEnabled": true }),
    )
    .await?;
    let began = events
        .iter()
        .any(|method| method.ends_with("downloadWillBegin"));
    let written = if kind == Kind::Local {
        downloaded_files(std::path::Path::new(&directory))
    } else {
        Vec::new()
    };
    if !began && written.is_empty() {
        provider.close(&page).await;
        provider.shutdown().await;
        return Err(Failure::Unsupported(format!(
            "under allow, the engine emitted no download event ({events:?}) and wrote no file: \
             Obscura {} acks Browser.setDownloadBehavior without acting on it and never \
             downloads to disk (D2126)",
            loams_sidebar_browser::PINNED_ENGINE_VERSION
        )));
    }
    provider.close(&page).await;
    provider.shutdown().await;
    Ok(format!("download began: event {began}, files {written:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_allowed_by_policy() {
    run("downloads_allowed", downloads_allowed_scenario).await;
}

// ------------------------------------------------------------ network policy

async fn network_policy_scenario(kind: Kind) -> Outcome {
    let denied = Fixture::start(None).await;
    let allowed = Fixture::start(Some(denied.origin())).await;
    let allowlist = NavigationAllowlist::new(origin(&allowed));

    // 1. A navigation to an origin outside the allow list never reaches the
    //    engine: Loams refuses it first.
    let target = url::Url::parse(&denied.url("/")).map_err(|error| error.to_string())?;
    ensure(allowlist.check(&target).is_err(), || {
        "the allow list let a second origin through".to_string()
    })?;
    // And the metadata endpoint is refused by Loams even though the engine's
    // private-network switch would open it.
    let metadata = EmbeddedOrigin::parse("http://169.254.169.254").map_err(|e| e.to_string())?;
    ensure(
        matches!(
            decide_origin(&metadata),
            Ok(PrivateNetworkDecision::Refused { .. }) | Err(_)
        ),
        || "the cloud metadata address was not refused".to_string(),
    )?;

    // 2. A subresource request from an allowed page to a denied origin is
    //    failed by the interceptor and never reaches the denied server.
    let mut provider = connect(kind, &allowed).await?;
    let page = provider.open(true).await?;
    navigate(&mut provider.client, &page, &allowed.url("/cross"), LOAD).await?;
    provider
        .client
        .call(
            "Fetch.enable",
            json!({ "patterns": [{ "urlPattern": "*", "requestStage": "Request" }] }),
            Some(&page.session_id),
        )
        .await
        .map_err(|error| format!("Fetch.enable at the request stage: {error}"))?;
    let decide = |paused: &page::Paused| match url::Url::parse(&paused.url) {
        Ok(url) if allowlist.check(&url).is_ok() => Verdict::Continue,
        _ => Verdict::Fail("AccessDenied"),
    };
    let (cross, decisions) = run_intercepted(
        &mut provider.client,
        &page,
        &fetch_into_r(&denied.url("/ping")),
        FETCH_RESULT,
        LOAD,
        decide,
    )
    .await?;
    ensure(cross.get("ok") == Some(&json!(false)), || {
        format!("a fetch to the denied origin succeeded: {cross}")
    })?;
    ensure(denied.seen_path("/ping").is_empty(), || {
        format!(
            "the denied origin received {} request(s)",
            denied.seen_path("/ping").len()
        )
    })?;
    ensure(
        decisions.iter().any(|(paused, verdict)| {
            paused.url.starts_with(&denied.origin()) && *verdict != Verdict::Continue
        }),
        || format!("the interceptor never saw the denied request: {decisions:?}"),
    )?;

    // 3. The same-origin request still works under the same interceptor.
    let (same, _) = run_intercepted(
        &mut provider.client,
        &page,
        &fetch_into_r(&allowed.url("/ping")),
        FETCH_RESULT,
        LOAD,
        decide,
    )
    .await?;
    ensure(same.get("ok") == Some(&json!(true)), || {
        format!("the allowed origin was blocked too: {same}")
    })?;
    let _ = provider
        .client
        .call("Fetch.disable", json!({}), Some(&page.session_id))
        .await;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok(
        "navigation and subresource to a denied origin refused; allowed origin unaffected"
            .to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_policy_denies_outside_the_allow_list() {
    run("network_policy_allowlist", network_policy_scenario).await;
}

// ------------------------------------------------------ cookies and profiles

async fn engine_cookie(
    provider: &mut Provider,
    page: &provider::PageHandle,
    fixture: &Fixture,
) -> Result<Option<ScopedSessionCookie>, Failure> {
    let response = provider
        .client
        .call("Storage.getCookies", json!({}), Some(&page.session_id))
        .await
        .map_err(|error| format!("Storage.getCookies: {error}"))?;
    let cookies = cookies_from_cdp(&response, &origin(fixture)).map_err(|e| e.to_string())?;
    Ok(cookies
        .into_iter()
        .find(|cookie| cookie.name() == "parity_session"))
}

async fn echoed_cookie(
    provider: &mut Provider,
    page: &provider::PageHandle,
    fixture: &Fixture,
) -> Result<String, Failure> {
    navigate(
        &mut provider.client,
        page,
        &fixture.url("/echo-cookie"),
        LOAD,
    )
    .await?;
    let text = eval(
        &mut provider.client,
        page,
        "document.querySelector('#result')?.textContent ?? ''",
    )
    .await?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

/// Ledger persistence: the path both providers share. Loams reads the app's
/// session cookie out of the engine, keeps it in the profile's ledger, and
/// re-injects it into a fresh context.
async fn cookie_ledger_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let key = ProfileKey::new("parity", "fixture").map_err(|e| e.to_string())?;
    let value = nonce("sess");

    let page = provider.open(true).await?;
    navigate(
        &mut provider.client,
        &page,
        &fixture.url(&format!("/set-cookie?v={value}")),
        LOAD,
    )
    .await?;
    let cookie = engine_cookie(&mut provider, &page, &fixture)
        .await?
        .ok_or("the engine did not keep the Set-Cookie from the fixture")?;
    ensure(cookie.value() == value, || {
        "the engine's cookie has a different value".to_string()
    })?;
    let mut ledger =
        CookieLedger::load_for(&provider.profile_dir, &key).map_err(|e| e.to_string())?;
    ledger.upsert(cookie);
    ledger
        .store(&provider.profile_dir, &origin(&fixture))
        .map_err(|e| e.to_string())?;
    provider.close(&page).await;

    // A fresh context starts without it...
    let fresh = provider.open(true).await?;
    let before = echoed_cookie(&mut provider, &fresh, &fixture).await?;
    let isolated = !before.contains(&value);
    // ...and has it after Loams re-injects from the ledger.
    let ledger = CookieLedger::load_for(&provider.profile_dir, &key).map_err(|e| e.to_string())?;
    let cookies: Vec<Value> = ledger
        .for_origin_cookies(&origin(&fixture))
        .iter()
        .map(|cookie| cookie.to_cdp_json())
        .collect();
    ensure(!cookies.is_empty(), || {
        "the ledger came back empty".to_string()
    })?;
    provider
        .client
        .call(
            "Storage.setCookies",
            json!({ "cookies": cookies }),
            Some(&fresh.session_id),
        )
        .await
        .map_err(|error| format!("Storage.setCookies: {error}"))?;
    let after = echoed_cookie(&mut provider, &fresh, &fixture).await?;
    ensure(after.contains(&format!("parity_session={value}")), || {
        format!("after re-injection the server saw {after:?}")
    })?;
    provider.close(&fresh).await;
    provider.shutdown().await;
    Ok(format!(
        "ledger round trip ok; new context started {}",
        if isolated {
            "clean"
        } else {
            "WITH the cookie (contexts share a jar)"
        }
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_ledger_persistence() {
    run("cookie_ledger_persistence", cookie_ledger_scenario).await;
}

/// Engine-native persistence: the cookie survives an engine restart through
/// `--storage-dir` alone, with no re-injection. Only a provider whose engine
/// the suite launches can be restarted; for the shared service it is `n/a`.
async fn profile_restart_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    if !provider.capabilities().engine_restart {
        provider.shutdown().await;
        return Err(Failure::NotApplicable(
            "the shared service's engine is not restarted per session; persistence is the \
             ledger's job there (cookie_ledger_persistence)"
                .to_string(),
        ));
    }
    let value = nonce("disk");
    let page = provider.open(false).await?;
    navigate(
        &mut provider.client,
        &page,
        &fixture.url(&format!("/set-cookie?v={value}")),
        LOAD,
    )
    .await?;
    provider.close(&page).await;
    provider.restart_engine().await?;
    let page = provider.open(false).await?;
    let echoed = echoed_cookie(&mut provider, &page, &fixture).await?;
    ensure(echoed.contains(&format!("parity_session={value}")), || {
        format!(
            "after a graceful restart on the same --storage-dir the server saw {echoed:?}; \
             the profile directory holds {}",
            profile_listing(&provider.profile_dir, &value)
        )
    })?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok("cookie survived an engine restart via --storage-dir".to_string())
}

/// The files in a profile directory, marking any that contain `needle`, for a
/// failure message that says where the engine did (or did not) write.
fn profile_listing(dir: &std::path::Path, needle: &str) -> String {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let holds = std::fs::read(&path)
                .map(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
                .unwrap_or(false);
            let name = path
                .strip_prefix(dir)
                .map(|relative| relative.display().to_string())
                .unwrap_or_default();
            out.push(if holds {
                format!("{name} (has the cookie)")
            } else {
                name
            });
        }
    }
    out.sort();
    if out.is_empty() {
        "no files".to_string()
    } else {
        out.join(", ")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_persists_across_engine_restart() {
    run("profile_restart_persistence", profile_restart_scenario).await;
}

// -------------------------------------------------------- credential boundary

async fn credential_boundary_scenario(kind: Kind) -> Outcome {
    let (canary, from_env) = match provider::canary() {
        Some(canary) => (canary, true),
        None => (nonce("loams_pat_parity_"), false),
    };
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let mut checks = Vec::new();

    // 1. The engine process never inherits it.
    if provider.capabilities().process_env && from_env {
        let pid = provider.engine_pid().ok_or("the local engine has no pid")?;
        let environ = std::fs::read(format!("/proc/{pid}/environ"))
            .map_err(|error| format!("reading the engine's environment: {error}"))?;
        ensure(!String::from_utf8_lossy(&environ).contains(&canary), || {
            "the engine process inherited the Loams token in its environment".to_string()
        })?;
        checks.push("process env");
    }

    // 2. The app gets only its own scoped session cookie, never the token.
    let session = ScopedSessionCookie::new(
        origin(&fixture),
        "app_session",
        nonce("app"),
        loams_sidebar_browser::CookieAttributes::session(&origin(&fixture)),
    )
    .map_err(|e| e.to_string())?;
    let page = provider.open(true).await?;
    provider
        .client
        .call(
            "Storage.setCookies",
            json!({ "cookies": [session.to_cdp_json()] }),
            Some(&page.session_id),
        )
        .await
        .map_err(|error| format!("Storage.setCookies: {error}"))?;
    navigate(&mut provider.client, &page, &fixture.url("/"), LOAD).await?;
    navigate(
        &mut provider.client,
        &page,
        &fixture.url("/echo-cookie"),
        LOAD,
    )
    .await?;

    // 3. Nothing the engine holds contains it.
    let response = provider
        .client
        .call("Storage.getCookies", json!({}), Some(&page.session_id))
        .await
        .map_err(|error| format!("Storage.getCookies: {error}"))?;
    ensure(!response.to_string().contains(&canary), || {
        "the engine's cookie jar holds the Loams token".to_string()
    })?;
    let cookies = cookies_from_cdp(&response, &origin(&fixture)).map_err(|e| e.to_string())?;
    assert_cookies_exclude(&cookies, &canary).map_err(|e| e.to_string())?;
    checks.push("cookie jar");

    // 4. Nothing the page can read contains it.
    let visible = eval(
        &mut provider.client,
        &page,
        "JSON.stringify({ c: document.cookie, l: Object.entries(localStorage), s: Object.entries(sessionStorage), u: navigator.userAgent, h: location.href })",
    )
    .await?;
    ensure(!visible.to_string().contains(&canary), || {
        "the page can read the Loams token".to_string()
    })?;
    checks.push("page-visible state");

    // 5. No request the engine sent carries it.
    let leaked = fixture
        .seen()
        .iter()
        .filter(|seen| seen.flattened().contains(&canary))
        .count();
    ensure(leaked == 0, || {
        format!("{leaked} request(s) to the fixture carried the Loams token")
    })?;
    ensure(!fixture.seen_path("/echo-cookie").is_empty(), || {
        "the fixture saw no traffic, so the header check proved nothing".to_string()
    })?;
    checks.push("request headers and bodies");

    // 6. Nothing on disk in the profile contains it.
    audit_profile(&provider.profile_dir, &canary).map_err(|e| e.to_string())?;
    checks.push("profile directory");

    provider.close(&page).await;
    provider.shutdown().await;
    Ok(format!(
        "token absent from: {}{}",
        checks.join(", "),
        if from_env {
            ""
        } else {
            " (no canary env: the process-env check was not exercised)"
        }
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_loams_token_reaches_the_engine() {
    run("credential_boundary", credential_boundary_scenario).await;
}

// ---------------------------------------------------- timeouts and budgets

async fn timeouts_scenario(kind: Kind) -> Outcome {
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;

    // 1. A per-call timeout fires on a call the engine cannot answer in time.
    let call_timeout = Duration::from_secs(2);
    let mut short = provider.second_client(call_timeout).await?;
    let hung = open_on(&mut short, true).await?;
    navigate(&mut short, &hung, &fixture.url("/"), LOAD).await?;
    let started = Instant::now();
    let result = short
        .call(
            "Runtime.evaluate",
            json!({ "expression": "fetch('/slow').then(r => r.text())", "awaitPromise": true }),
            Some(&hung.session_id),
        )
        .await;
    let elapsed = started.elapsed();
    match result {
        Err(error) if page::is_timeout(&error) => {}
        Err(error) => return Err(format!("expected a timeout, got {error}").into()),
        Ok(value) => return Err(format!("a 45 s request answered within 2 s: {value}").into()),
    }
    ensure(
        (Duration::from_millis(1500)..=Duration::from_secs(8)).contains(&elapsed),
        || format!("the 2 s timeout fired after {} ms", elapsed.as_millis()),
    )?;

    // 2. A Loams-side navigation budget cuts a page that never finishes.
    let page = provider.open(true).await?;
    let budget = Duration::from_secs(3);
    let started = Instant::now();
    let cut = tokio::time::timeout(
        budget,
        navigate(&mut provider.client, &page, &fixture.url("/slow"), LOAD),
    )
    .await;
    let elapsed = started.elapsed();
    ensure(cut.is_err(), || {
        format!("a 45 s page loaded within the 3 s budget: {cut:?}")
    })?;
    ensure(elapsed <= budget + Duration::from_secs(2), || {
        format!("the 3 s budget took {} ms to fire", elapsed.as_millis())
    })?;
    provider.close(&page).await;

    // 3. Neither left the provider wedged: a fresh page loads normally.
    let fresh = provider.open(true).await?;
    navigate(&mut provider.client, &fresh, &fixture.url("/"), LOAD).await?;
    let title = eval(&mut provider.client, &fresh, "document.title").await?;
    ensure(title == json!("Parity fixture"), || {
        format!("after the timeouts, a fresh page has title {title}")
    })?;
    provider.close(&fresh).await;
    close_on(&mut short, &hung).await;
    drop(short);
    provider.shutdown().await;
    Ok(format!(
        "call timeout at {} ms; navigation budget cut at {} ms; provider recovered",
        call_timeout.as_millis(),
        elapsed.as_millis()
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeouts_and_budgets() {
    run("timeouts_budgets", timeouts_scenario).await;
}

// ----------------------------------------------------------------- recording

async fn recording_scenario(kind: Kind) -> Outcome {
    let ffmpeg = resolve_ffmpeg();
    let probe = tokio::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .await;
    if !probe.is_ok_and(|output| output.status.success()) {
        return Err(Failure::Unconfigured(format!(
            "no encoder at {}; set LOAMS_FFMPEG_BIN or install ffmpeg",
            ffmpeg.display()
        )));
    }
    let fixture = Fixture::start(None).await;
    let mut provider = connect(kind, &fixture).await?;
    let page = provider.open(true).await?;
    navigate(&mut provider.client, &page, &fixture.url("/animate"), LOAD).await?;
    let output = provider.profile_dir.join("parity-recording.webm");
    let recorder = VideoRecorder::new(
        RecorderConfig::new(output)
            .with_container(Container::WebM)
            .with_ffmpeg(ffmpeg),
    );
    recorder.start().await.map_err(|e| e.to_string())?;
    recorder
        .start_screencast(&mut provider.client, &page.session_id, 640, 480)
        .await
        .map_err(|error| format!("Page.startScreencast: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut received = 0_u64;
    let mut recorded = 0_u64;
    let mut dims = (0, 0);
    while recorded < 5 && Instant::now() < deadline {
        let Some(event) =
            page::next_event(&mut provider.client, Duration::from_millis(500)).await?
        else {
            continue;
        };
        if event.method != "Page.screencastFrame" {
            continue;
        }
        let stream_id = event
            .param("sessionId")
            .and_then(Value::as_i64)
            .ok_or("a screencast frame without an integer sessionId")?;
        let data = event
            .str_param("data")
            .ok_or("a screencast frame without data")?;
        received += 1;
        let frame = decode_frame(stream_id, data, FrameFormat::Png, received)
            .map_err(|error| format!("decoding frame {received}: {error}"))?;
        dims = (frame.width, frame.height);
        provider
            .client
            .call(
                "Page.screencastFrameAck",
                frame.ack_params(),
                Some(&page.session_id),
            )
            .await
            .map_err(|error| format!("Page.screencastFrameAck: {error}"))?;
        if let FrameDisposition::Recorded { .. } = recorder
            .write_frame(&frame, Instant::now())
            .await
            .map_err(|e| e.to_string())?
        {
            recorded += 1;
        }
    }
    let summary = recorder
        .stop_all(&mut provider.client, &page.session_id)
        .await
        .map_err(|error| format!("finalising the recording ({received} frames received): {error}"))?
        .ok_or("the recorder said nothing was recording")?;
    ensure(summary.frames >= 1 && summary.bytes > 0, || {
        format!("an empty recording: {summary}")
    })?;
    let on_disk = std::fs::metadata(&summary.path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    ensure(on_disk == summary.bytes, || {
        format!(
            "the summary says {} bytes, the file has {on_disk}",
            summary.bytes
        )
    })?;
    provider.close(&page).await;
    provider.shutdown().await;
    Ok(format!(
        "{received} frames received ({}x{}), {} recorded into {} bytes of WebM",
        dims.0, dims.1, summary.frames, summary.bytes
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recording_produces_a_video() {
    run("recording", recording_scenario).await;
}
