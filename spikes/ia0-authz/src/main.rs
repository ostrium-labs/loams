//! `ia0-authz <command>`:
//!
//! * `serve`       the spike service (in the compose network, profile `spike`)
//! * `scenario`    Task 3: allow and deny for a person and an agent, end to end
//! * `revocation`  the agent revocation timings (tuple delete, machine user deactivated)
//! * `events N`    Task 4: N org-member add/remove pairs through Actions v2
//! * `bench N`     Task 5: Check latency (run it inside the compose network)
//! * `agent-token` print a fresh agent access token (for curl)

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use ia0_authz::config::Config;
use ia0_authz::fga::{self, Consistency, Fga};
use ia0_authz::zitadel::{MachineKey, Zitadel, peek};
use ia0_authz::{percentiles, server};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = Config::load()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    match args.first().map(String::as_str) {
        Some("serve") => {
            let state = ia0_authz::state(&cfg).await?;
            server::serve(state, cfg.or("IA0_LISTEN", "127.0.0.1:18090")).await
        }
        Some("scenario") => scenario(&cfg).await,
        Some("revocation") => revocation(&cfg).await,
        Some("events") => events(&cfg, n(1, 100), n(2, 0)).await,
        Some("bench") => bench(&cfg, n(1, 10_000)).await,
        Some("agent-token") => {
            let t = agent_token(&cfg).await?;
            println!("{}", t.access_token);
            Ok(())
        }
        _ => bail!("usage: ia0-authz serve|scenario|revocation|events [N]|bench [N]|agent-token"),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn secret(cfg: &Config, name: &str) -> Result<String> {
    let p = cfg.base_dir().join(".secrets").join(name);
    Ok(std::fs::read_to_string(&p)
        .with_context(|| format!("read {}", p.display()))?
        .trim()
        .to_owned())
}

fn machine_key(cfg: &Config) -> Result<MachineKey> {
    Ok(serde_json::from_str(&std::fs::read_to_string(
        cfg.path("IA0_AGENT_KEY")?,
    )?)?)
}

async fn agent_token(cfg: &Config) -> Result<ia0_authz::zitadel::TokenResponse> {
    Zitadel::new(cfg.req("IA0_ISSUER")?)
        .agent_token(&machine_key(cfg)?, cfg.req("IA0_AUDIENCE")?)
        .await
}

/// The driver's view of the spike service.
struct Svc {
    base: String,
    admin: String,
    http: reqwest::Client,
}

impl Svc {
    fn new(cfg: &Config) -> Self {
        Self {
            base: cfg.or("IA0_SPIKE_URL", "http://127.0.0.1:18090").to_owned(),
            admin: cfg.or("IA0_ADMIN_TOKEN", "").to_owned(),
            http: reqwest::Client::new(),
        }
    }

    async fn call(&self, token: &str, method: Method, path: &str) -> Result<(StatusCode, Value)> {
        let res = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token)
            .send()
            .await?;
        let s = res.status();
        let v = res.json().await.unwrap_or(Value::Null);
        Ok((s, v))
    }

    async fn tuples(
        &self,
        writes: &[(&str, &str, &str)],
        deletes: &[(&str, &str, &str)],
    ) -> Result<()> {
        let res = self
            .http
            .post(format!("{}/admin/tuples", self.base))
            .bearer_auth(&self.admin)
            .json(&json!({ "writes": writes, "deletes": deletes }))
            .send()
            .await?;
        if !res.status().is_success() {
            bail!("admin/tuples: {} {}", res.status(), res.text().await?);
        }
        Ok(())
    }
}

async fn direct_fga(cfg: &Config) -> Result<Fga> {
    Fga::connect(
        cfg.req("IA0_FGA_GRPC")?,
        cfg.req("IA0_STORE")?,
        cfg.req("IA0_MODEL")?,
        Duration::ZERO,
    )
    .await
}

async fn seed(cfg: &Config, svc: &Svc) -> Result<()> {
    let org = format!("org:{}", cfg.req("IA0_ORG")?);
    let alice = format!("user:{}", cfg.req("IA0_ALICE")?);
    let agent = format!("agent:{}", cfg.req("IA0_AGENT")?);
    svc.tuples(
        &[
            (&org, "org", "project:p1"),
            ("project:p1", "project", "environment:dev"),
            ("project:p1", "project", "environment:prod"),
            ("environment:dev", "environment", "deployment:dev-web"),
            ("environment:prod", "environment", "deployment:prod-web"),
            ("environment:dev", "environment", "database:dev-db"),
            (&alice, "owner", &agent),
            ("project:p1", "project", &agent),
        ],
        &[],
    )
    .await
}

fn row(step: &str, who: &str, what: &str, got: StatusCode, want: StatusCode) -> bool {
    let ok = got == want;
    println!(
        "| {step} | {who} | {what} | {} | {} |",
        got.as_u16(),
        if ok { "pass" } else { "FAIL" }
    );
    ok
}

/// Task 3's run, printed as a Markdown table for the report.
async fn scenario(cfg: &Config) -> Result<()> {
    let z = Zitadel::new(cfg.req("IA0_ISSUER")?);
    let svc = Svc::new(cfg);
    let project = cfg.req("IA0_AUDIENCE")?;
    seed(cfg, &svc).await?;
    let alice_id = cfg.req("IA0_ALICE")?;
    let agent_id = cfg.req("IA0_AGENT")?;
    let alice = format!("user:{alice_id}");
    let agent = format!("agent:{agent_id}");
    let pw = std::env::var("ALICE_PASSWORD")
        .ok()
        .or_else(|| read_env_value(cfg, "ALICE_PASSWORD"))
        .context("ALICE_PASSWORD")?;

    let t0 = Instant::now();
    let human = z
        .human_login(
            &secret(cfg, "login-client.pat")?,
            cfg.req("IA0_CLIENT_ID")?,
            project,
            "alice",
            &pw,
        )
        .await?;
    let human_ms = t0.elapsed().as_millis();
    let t0 = Instant::now();
    let robot = agent_token(cfg).await?;
    let agent_ms = t0.elapsed().as_millis();
    let hc = peek(&human.access_token);
    let ac = peek(&robot.access_token);
    println!(
        "alice token ({human_ms} ms): aud={} org={} exp-iat={}s",
        hc["aud"],
        hc["urn:zitadel:iam:user:resourceowner:id"],
        hc["exp"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(hc["iat"].as_u64().unwrap_or(0))
    );
    println!(
        "agent token ({agent_ms} ms): aud={} org={} exp-iat={}s expires_in={}",
        ac["aud"],
        ac["urn:zitadel:iam:user:resourceowner:id"],
        ac["exp"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(ac["iat"].as_u64().unwrap_or(0)),
        robot.expires_in
    );
    println!();
    println!("| # | Principal | Request | Status | Result |");
    println!("|---|---|---|---|---|");
    let (h, a) = (&human.access_token, &robot.access_token);
    let mut ok = true;
    let s = |r: Result<(StatusCode, Value)>| r.map(|x| x.0).unwrap_or(StatusCode::IM_A_TEAPOT);
    ok &= row(
        "1",
        "alice, no tuple",
        "GET /deployments/dev-web",
        s(svc.call(h, Method::GET, "/deployments/dev-web").await),
        StatusCode::FORBIDDEN,
    );
    svc.tuples(&[(&alice, "viewer", "project:p1")], &[]).await?;
    ok &= row(
        "2a",
        "alice, viewer on p1",
        "GET /deployments/dev-web",
        s(svc.call(h, Method::GET, "/deployments/dev-web").await),
        StatusCode::OK,
    );
    ok &= row(
        "2b",
        "alice, viewer on p1",
        "DELETE /deployments/dev-web",
        s(svc.call(h, Method::DELETE, "/deployments/dev-web").await),
        StatusCode::FORBIDDEN,
    );
    ok &= row(
        "3a",
        "agent-1, no grant (owner alice)",
        "GET /deployments/dev-web",
        s(svc.call(a, Method::GET, "/deployments/dev-web").await),
        StatusCode::FORBIDDEN,
    );
    svc.tuples(&[(&agent, "editor", "environment:dev")], &[])
        .await?;
    ok &= row(
        "3b",
        "agent-1, editor on dev",
        "GET /deployments/dev-web",
        s(svc.call(a, Method::GET, "/deployments/dev-web").await),
        StatusCode::OK,
    );
    ok &= row(
        "3c",
        "agent-1, editor on dev",
        "POST /deployments/dev-web/restart",
        s(svc
            .call(a, Method::POST, "/deployments/dev-web/restart")
            .await),
        StatusCode::OK,
    );
    ok &= row(
        "3d",
        "agent-1, editor on dev",
        "GET /deployments/prod-web",
        s(svc.call(a, Method::GET, "/deployments/prod-web").await),
        StatusCode::FORBIDDEN,
    );
    ok &= row(
        "3e",
        "agent-1, editor on dev",
        "DELETE /deployments/dev-web",
        s(svc.call(a, Method::DELETE, "/deployments/dev-web").await),
        StatusCode::FORBIDDEN,
    );
    let (st, list) = svc.call(a, Method::GET, "/deployments").await?;
    println!(
        "| 3f | agent-1, editor on dev | GET /deployments (ListObjects) | {} | {} |",
        st.as_u16(),
        list["objects"]
    );
    ok &= list["objects"] == json!(["deployment:dev-web"]);
    ok &= row(
        "3g",
        "agent-1",
        "GET /admin-only-route (undeclared)",
        s(svc.call(a, Method::GET, "/secrets").await),
        StatusCode::FORBIDDEN,
    );

    // Step 4: delete the grant directly in OpenFGA (as another writer would), so the service's
    // allow cache is not cleared; the HIGHER_CONSISTENCY route sees it at once.
    svc.tuples(&[(&agent, "deployer", "environment:dev")], &[])
        .await?;
    ok &= row(
        "4a",
        "agent-1, deployer on dev",
        "DELETE /deployments/dev-web",
        s(svc.call(a, Method::DELETE, "/deployments/dev-web").await),
        StatusCode::OK,
    );
    let _ = svc.call(a, Method::GET, "/deployments/dev-web").await?; // warm the allow cache
    direct_fga(cfg)
        .await?
        .write(
            Vec::new(),
            vec![
                fga::key(&agent, "deployer", "environment:dev"),
                fga::key(&agent, "editor", "environment:dev"),
            ],
        )
        .await?;
    ok &= row(
        "4b",
        "agent-1, grant deleted",
        "DELETE /deployments/dev-web (HIGHER_CONSISTENCY, uncached)",
        s(svc.call(a, Method::DELETE, "/deployments/dev-web").await),
        StatusCode::FORBIDDEN,
    );
    let (st, _) = svc.call(a, Method::GET, "/deployments/dev-web").await?;
    println!(
        "| 4c | agent-1, grant deleted | GET /deployments/dev-web (MINIMIZE_LATENCY, cached allow) | {} | {} |",
        st.as_u16(),
        if st == StatusCode::OK {
            "stale allow, expected within the 5 s TTL"
        } else {
            "denied"
        }
    );
    let t0 = Instant::now();
    loop {
        let (st, _) = svc.call(a, Method::GET, "/deployments/dev-web").await?;
        if st == StatusCode::FORBIDDEN {
            break;
        }
        if t0.elapsed() > Duration::from_secs(15) {
            bail!("cached allow never expired");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!(
        "| 4d | agent-1, grant deleted | GET /deployments/dev-web until denied | 403 | after {} ms (cache TTL) |",
        t0.elapsed().as_millis()
    );

    // Audience and issuer.
    let no_aud = Zitadel::new(cfg.req("IA0_ISSUER")?)
        .agent_token_scoped(&machine_key(cfg)?, "openid")
        .await?;
    ok &= row(
        "5a",
        "agent-1, token without the project audience",
        "GET /whoami",
        s(svc.call(&no_aud.access_token, Method::GET, "/whoami").await),
        StatusCode::UNAUTHORIZED,
    );
    ok &= row(
        "5b",
        "nobody",
        "GET /whoami with a forged token",
        s(svc
            .call(
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln",
                Method::GET,
                "/whoami",
            )
            .await),
        StatusCode::UNAUTHORIZED,
    );

    // agent_cannot_write_grants: the model refuses agent subjects on people-only relations.
    let f = direct_fga(cfg).await?;
    let org = format!("org:{}", cfg.req("IA0_ORG")?);
    for (rel, obj) in [
        ("billing_admin", org.as_str()),
        ("meter_admin", "platform:default"),
        ("operator", "platform:default"),
    ] {
        let r = f.write_strict(vec![fga::tuple(&agent, rel, obj)]).await;
        let refused = r.as_ref().is_err_and(|e| format!("{e:?}").contains("type"));
        println!(
            "| 6 | write {rel}@{agent} | model write | {} | {} |",
            if r.is_err() { "refused" } else { "accepted" },
            if refused { "pass" } else { "FAIL" }
        );
        ok &= refused;
    }
    if !ok {
        bail!("scenario failed");
    }
    Ok(())
}

fn read_env_value(cfg: &Config, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(cfg.base_dir().join(".env")).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_owned))
}

/// The agent revocation timings: (a) the grant's tuple deleted, (b) the machine user deactivated
/// in Zitadel, observed through Actions v2 -> the spike's sync -> the next request.
async fn revocation(cfg: &Config) -> Result<()> {
    let z = Zitadel::new(cfg.req("IA0_ISSUER")?);
    let svc = Svc::new(cfg);
    let pat = secret(cfg, "admin.pat")?;
    let org = cfg.req("IA0_ORG")?;
    let agent_id = cfg.req("IA0_AGENT")?;
    let agent = format!("agent:{agent_id}");
    seed(cfg, &svc).await?;
    let mut rows = Vec::new();
    for round in 0..10 {
        svc.tuples(&[(&agent, "editor", "environment:dev")], &[])
            .await?;
        let tok = agent_token(cfg).await?.access_token;
        let (st, _) = svc.call(&tok, Method::GET, "/deployments/dev-web").await?;
        if st != StatusCode::OK {
            bail!("round {round}: granted agent denied ({st})");
        }
        // Deactivate and time: token endpoint refusal, then the request denial.
        let t0 = Instant::now();
        z.api_org(
            &pat,
            Method::POST,
            &format!("/management/v1/users/{agent_id}/_deactivate"),
            Some(json!({})),
            Some(org),
        )
        .await?;
        let api_ms = t0.elapsed().as_millis();
        let refused = agent_token(cfg).await.is_err();
        let mut denied_ms = None;
        while t0.elapsed() < Duration::from_secs(30) {
            let (st, _) = svc.call(&tok, Method::GET, "/deployments/dev-web").await?;
            if st != StatusCode::OK {
                denied_ms = Some(t0.elapsed().as_millis());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let f = direct_fga(cfg).await?;
        let mut tuple_gone_ms = None;
        while t0.elapsed() < Duration::from_secs(30) {
            if !f
                .check(
                    &agent,
                    "editor",
                    "environment:dev",
                    Consistency::HigherConsistency,
                    None,
                )
                .await?
                .allowed
            {
                tuple_gone_ms = Some(t0.elapsed().as_millis());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        rows.push((round, api_ms, refused, denied_ms, tuple_gone_ms));
        z.api_org(
            &pat,
            Method::POST,
            &format!("/management/v1/users/{agent_id}/_reactivate"),
            Some(json!({})),
            Some(org),
        )
        .await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    println!(
        "| Round | Deactivate call | New token refused | Old token denied after | Grant tuple deleted after |"
    );
    println!("|---|---|---|---|---|");
    for (r, api, refused, denied, left) in rows {
        let ms = |d: Option<u128>| d.map_or("never (30 s)".to_owned(), |d| format!("{d} ms"));
        println!(
            "| {r} | {api} ms | {} | {} | {} |",
            if refused { "yes" } else { "NO" },
            ms(denied),
            ms(left)
        );
    }
    Ok(())
}

/// Task 4: N add/remove pairs of alice as an org member, through Actions v2.
async fn events(cfg: &Config, pairs: usize, pace_ms: usize) -> Result<()> {
    let z = Zitadel::new(cfg.req("IA0_ISSUER")?);
    let svc = Svc::new(cfg);
    let pat = secret(cfg, "admin.pat")?;
    let org = cfg.req("IA0_ORG")?;
    let alice = cfg.req("IA0_ALICE")?;
    let admin = |m: Method, p: &str| {
        svc.http
            .request(m, format!("{}{p}", svc.base))
            .bearer_auth(&svc.admin)
    };
    // Start from "not a member", then clear the log.
    let _ = z
        .api_org(
            &pat,
            Method::DELETE,
            &format!("/management/v1/orgs/me/members/{alice}"),
            None,
            Some(org),
        )
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    admin(Method::DELETE, "/admin/events").send().await?;
    let mut calls = Vec::new();
    for _ in 0..pairs {
        let t = now_ms();
        z.api_org(
            &pat,
            Method::POST,
            "/management/v1/orgs/me/members",
            Some(json!({ "userId": alice, "roles": ["ORG_USER_MANAGER"] })),
            Some(org),
        )
        .await?;
        calls.push(("org.member.added", t, now_ms()));
        let t = now_ms();
        z.api_org(
            &pat,
            Method::DELETE,
            &format!("/management/v1/orgs/me/members/{alice}"),
            None,
            Some(org),
        )
        .await?;
        calls.push(("org.member.removed", t, now_ms()));
        if pace_ms > 0 {
            tokio::time::sleep(Duration::from_millis(pace_ms as u64)).await;
        }
    }
    let sent_done = Instant::now();
    // Wait for every event (or 120 s), then 5 s more for stragglers and duplicates.
    let mut log: Vec<Value>;
    loop {
        log = admin(Method::GET, "/admin/events")
            .send()
            .await?
            .json()
            .await?;
        let got = log
            .iter()
            .filter(|e| {
                e["event_type"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("org.member"))
            })
            .count();
        if got >= calls.len() || sent_done.elapsed() > Duration::from_secs(120) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let drained_ms = sent_done.elapsed().as_millis();
    tokio::time::sleep(Duration::from_secs(5)).await;
    log = admin(Method::GET, "/admin/events")
        .send()
        .await?
        .json()
        .await?;
    println!(
        "pairs={pairs} pace={pace_ms} ms; last event received {drained_ms} ms after the last call returned"
    );
    let member: Vec<&Value> = log
        .iter()
        .filter(|e| {
            e["event_type"]
                .as_str()
                .is_some_and(|t| t.starts_with("org.member"))
        })
        .collect();
    let mut out_of_order = 0;
    let mut last = 0;
    for e in &member {
        let s = e["sequence"].as_u64().unwrap_or(0);
        if s < last {
            out_of_order += 1;
        }
        last = last.max(s);
    }
    // Latency: call return -> receipt, matched in order (the i-th call to the i-th event).
    let mut lat: Vec<u128> = Vec::new();
    for (i, e) in member.iter().enumerate() {
        if let Some((_, _, returned)) = calls.get(i) {
            let rec = e["received_unix_ms"].as_u64().unwrap_or(0) as u128;
            lat.push(rec.saturating_sub(*returned) * 1000);
        }
    }
    let fga = direct_fga(cfg).await?;
    let present = fga
        .check(
            &format!("user:{alice}"),
            "member",
            &format!("org:{org}"),
            Consistency::HigherConsistency,
            None,
        )
        .await?
        .allowed;
    let listed = z
        .api_org(
            &pat,
            Method::POST,
            "/management/v1/orgs/me/members/_search",
            Some(json!({})),
            Some(org),
        )
        .await?["result"]
        .as_array()
        .is_some_and(|a| a.iter().any(|m| m["userId"] == alice));
    let dupes = member.len() as i64
        - member
            .iter()
            .map(|e| e["sequence"].as_u64())
            .collect::<std::collections::HashSet<_>>()
            .len() as i64;
    println!(
        "calls={} received={} duplicates={dupes} missed={} out_of_order={out_of_order} stale_skipped={}",
        calls.len(),
        member.len(),
        calls.len() as i64 - member.len() as i64 + dupes,
        member.iter().filter(|e| e["stale"] == true).count()
    );
    if !lat.is_empty() {
        let (p50, p95, p99, mean) = percentiles(&mut lat);
        println!(
            "call-return -> webhook receipt: p50={p50:.0} ms p95={p95:.0} ms p99={p99:.0} ms mean={mean:.0} ms max={:.0} ms",
            *lat.last().unwrap() as f64 / 1000.0
        );
    }
    println!(
        "final state: zitadel lists alice as member={listed}, openfga tuple present={present} (drift={})",
        listed != present
    );
    let other: Vec<&Value> = log
        .iter()
        .filter(|e| {
            !e["event_type"]
                .as_str()
                .is_some_and(|t| t.starts_with("org.member"))
        })
        .collect();
    if !other.is_empty() {
        println!(
            "other events received: {}",
            other
                .iter()
                .map(|e| e["event_type"].as_str().unwrap_or("?"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// Task 5: Check latency against OpenFGA. Run inside the compose network for loopback numbers.
async fn bench(cfg: &Config, n: usize) -> Result<()> {
    let fga = Fga::connect(
        cfg.req("IA0_FGA_GRPC")?,
        cfg.req("IA0_STORE")?,
        cfg.req("IA0_MODEL")?,
        Duration::from_secs(5),
    )
    .await?;
    let org = format!("org:{}", cfg.req("IA0_ORG")?);
    // Seed: 50 deployments in env bench, an agent with editor on it, a person who is org owner.
    let mut writes = vec![
        fga::tuple(&org, "org", "project:bench"),
        fga::tuple("project:bench", "project", "environment:bench"),
    ];
    for i in 0..50 {
        writes.push(fga::tuple(
            "environment:bench",
            "environment",
            &format!("deployment:b{i}"),
        ));
    }
    writes.push(fga::tuple("agent:bench", "editor", "environment:bench"));
    writes.push(fga::tuple("user:bench-owner", "owner", &org));
    fga.write(writes, Vec::new()).await?;

    println!("| Call | Consistency | Path | n | p50 ms | p95 ms | p99 ms | mean ms |");
    println!("|---|---|---|---|---|---|---|---|");
    let cases = [
        (
            "agent:bench",
            "can_view",
            "agent editor on environment (3 hops)",
        ),
        (
            "user:bench-owner",
            "can_destroy",
            "person via org owner (5 hops)",
        ),
        ("agent:nobody", "can_view", "deny, no tuple"),
    ];
    for consistency in [Consistency::MinimizeLatency, Consistency::HigherConsistency] {
        for (user, rel, label) in cases {
            let mut s = Vec::with_capacity(n);
            for i in 0..n {
                let obj = format!("deployment:b{}", i % 50);
                let t = Instant::now();
                fga.check(user, rel, &obj, consistency, Some(fga::context(json!({}))?))
                    .await?; // context set: bypasses the spike cache
                s.push(t.elapsed().as_micros());
            }
            let (p50, p95, p99, mean) = percentiles(&mut s);
            println!(
                "| Check | {consistency:?} | {label} | {n} | {p50:.2} | {p95:.2} | {p99:.2} | {mean:.2} |"
            );
        }
    }
    // The spike's in-process allow cache (TTL 5 s).
    let mut s = Vec::with_capacity(n);
    for i in 0..n {
        let obj = format!("deployment:b{}", i % 50);
        let t = Instant::now();
        fga.check(
            "agent:bench",
            "can_view",
            &obj,
            Consistency::MinimizeLatency,
            None,
        )
        .await?;
        s.push(t.elapsed().as_micros());
    }
    let (p50, p95, p99, mean) = percentiles(&mut s);
    println!(
        "| Check | cached (in-process, TTL 5 s) | agent editor on environment | {n} | {p50:.3} | {p95:.3} | {p99:.3} | {mean:.3} |"
    );

    // 50 checks: one BatchCheck vs 50 sequential vs 50 concurrent Checks.
    let batches = (n / 50).max(1);
    let items: Vec<(String, String, String)> = (0..50)
        .map(|i| {
            (
                "agent:bench".to_owned(),
                "can_view".to_owned(),
                format!("deployment:b{i}"),
            )
        })
        .collect();
    for consistency in [Consistency::MinimizeLatency, Consistency::HigherConsistency] {
        let mut s = Vec::with_capacity(batches);
        for _ in 0..batches {
            let t = Instant::now();
            let r = fga.batch_check(&items, consistency).await?;
            s.push(t.elapsed().as_micros());
            assert!(r.iter().all(|a| *a));
        }
        let (p50, p95, p99, mean) = percentiles(&mut s);
        println!(
            "| BatchCheck x50 | {consistency:?} | agent editor on environment | {batches} | {p50:.2} | {p95:.2} | {p99:.2} | {mean:.2} |"
        );
        let mut s = Vec::with_capacity(batches);
        for _ in 0..batches {
            let t = Instant::now();
            let futs = items.iter().map(|(u, r, o)| {
                fga.check(u, r, o, consistency, Some(fga::context(json!({})).unwrap()))
            });
            for r in futures::future::join_all(futs).await {
                r?;
            }
            s.push(t.elapsed().as_micros());
        }
        let (p50, p95, p99, mean) = percentiles(&mut s);
        println!(
            "| 50 concurrent Checks | {consistency:?} | agent editor on environment | {batches} | {p50:.2} | {p95:.2} | {p99:.2} | {mean:.2} |"
        );
    }
    Ok(())
}
