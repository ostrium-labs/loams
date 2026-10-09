//! The link CLI's verbs. `loams-agentd` exposes only `bot-acp` (as
//! `loams-agentd loams bot-acp`, plan DD1 ruling T1-2); the other verbs are
//! reachable only through [`run`] and go with the WorkOS-era code in Task 3.
//! Hand-parsed: five verbs do not need a second argument parser.
//!
//! ```text
//! status            instance, API versions and sign-in methods
//! login             Authentik sign-in in the system browser
//! logout            forget the stored refresh token
//! bot "<message>"   one message to Loams Bot over A2A
//! bot-acp           serve Loams Bot to the sessions engine over ACP (stdio)
//! mock [ADDR]       run the in-process mock (default 127.0.0.1:8084)
//! ```
//!
//! `LOAMS_URL`, `LOAMS_MOCK=1`, `LOAMS_BOT_URL` and `LOAMS_OIDC_ISSUER` configure it
//! (see [`crate::config`]). With `LOAMS_MOCK=1` every verb runs against an
//! in-process mock, so the whole path works with no server.

use std::sync::Arc;

use anyhow::{Context as _, bail};
use tokio::io::BufReader;

use crate::a2a::{A2aClient, JsonRpcA2aClient, Message, MockA2aClient, Part, Reply, Role};
use crate::auth::{self, KeyringStore, MemoryStore, OidcClient, SystemBrowser, TokenStore};
use crate::client::LoamsClient;
use crate::config::LoamsConfig;
use crate::mock::MockServer;
use crate::proto::loams::instance::v1::{GetInstanceResponse, SignInKind};

/// Usage text for the link CLI with no or unknown arguments.
pub const USAGE: &str = "usage: <status|login|logout|bot \"<message>\"|bot-acp|mock [ADDR]>";

/// The loopback address `mock` listens on by default (beside `loams-apps-mock`).
pub const DEFAULT_MOCK_ADDR: &str = "127.0.0.1:8084";

/// True for the verb whose stdout carries a protocol, so the caller must send
/// logs to stderr.
#[must_use]
pub fn owns_stdout(args: &[String]) -> bool {
    args.first().map(String::as_str) == Some("bot-acp")
}

/// Runs a subcommand and returns the process exit code.
///
/// # Errors
///
/// Anything that stops the command; `main` prints it and exits non-zero.
pub async fn run(args: Vec<String>) -> anyhow::Result<i32> {
    let config = LoamsConfig::from_env();
    let verb = args.first().map(String::as_str);
    match verb {
        Some("status") => status(&config).await,
        Some("login") => login(&config).await,
        Some("logout") => logout(&config).await,
        Some("bot") => {
            let text = args[1..].join(" ");
            if text.trim().is_empty() {
                bail!("{USAGE}");
            }
            bot(&config, &text).await
        }
        Some("bot-acp") => bot_acp(&config).await,
        Some("mock") => mock(args.get(1).map_or(DEFAULT_MOCK_ADDR, String::as_str)).await,
        _ => {
            eprintln!("{USAGE}");
            Ok(2)
        }
    }
}

/// Connects to the configured server, or starts the mock in mock mode. The
/// returned guard keeps a mock alive.
async fn connect(config: &LoamsConfig) -> anyhow::Result<(LoamsClient, Option<MockServer>)> {
    if config.mock {
        let mock = MockServer::start("127.0.0.1:0".parse()?).await?;
        eprintln!("using the in-process mock at {}", mock.url());
        Ok((LoamsClient::connect(&mock.url())?, Some(mock)))
    } else {
        Ok((LoamsClient::connect(&config.server_url)?, None))
    }
}

async fn status(config: &LoamsConfig) -> anyhow::Result<i32> {
    let (client, _mock) = connect(config).await?;
    let instance = client.get_instance().await.context("GetInstance")?;
    println!("instance   {} ({})", instance.name, instance.instance_id);
    println!("server     {}", instance.server_version);
    println!("api        {}", instance.api_versions.join(", "));
    for method in &instance.sign_in_methods {
        println!(
            "sign-in    {:?} {}",
            method
                .kind
                .as_known()
                .unwrap_or(SignInKind::SIGN_IN_KIND_UNSPECIFIED),
            method.issuer
        );
    }
    let me = client.who_am_i(None).await;
    match me {
        Ok(me) => println!("you        {}", me.principal.display_name),
        Err(error) => println!("you        not signed in ({error})"),
    }
    Ok(0)
}

fn store() -> Box<dyn TokenStore> {
    match keyring::Entry::store_status() {
        Ok(()) => Box::new(KeyringStore::new(crate::brand::KEYRING_SERVICE)),
        Err(error) => {
            eprintln!("no OS keychain ({error}); the sign-in will last this process only");
            Box::new(MemoryStore::default())
        }
    }
}

fn authentik_issuer(
    instance: &GetInstanceResponse,
    config: &LoamsConfig,
) -> Option<(String, String)> {
    let method = instance
        .sign_in_methods
        .iter()
        .find(|m| m.kind.as_known() == Some(SignInKind::SIGN_IN_KIND_AUTHENTIK))?;
    let issuer = config
        .oidc_issuer
        .clone()
        .unwrap_or_else(|| method.issuer.clone());
    let client_id = if method.client_id.is_empty() {
        crate::brand::OIDC_CLIENT_ID.to_owned()
    } else {
        method.client_id.clone()
    };
    Some((issuer, client_id))
}

async fn login(config: &LoamsConfig) -> anyhow::Result<i32> {
    let (client, _mock) = connect(config).await?;
    let instance = client.get_instance().await.context("GetInstance")?;
    let Some((issuer, client_id)) = authentik_issuer(&instance, config) else {
        println!(
            "{} needs no sign-in (a local stack before the auth plan).",
            instance.name
        );
        return Ok(0);
    };
    let oidc = OidcClient::discover(client_id, &issuer)
        .await
        .context("OIDC discovery")?;
    println!("Opening your browser to sign in at {issuer} ...");
    let store = store();
    let tokens = auth::sign_in(
        &oidc,
        store.as_ref(),
        &instance.instance_id,
        &SystemBrowser,
        auth::SIGN_IN_TIMEOUT,
    )
    .await
    .context("sign-in")?;
    println!(
        "Signed in. Access token valid for {}s; refresh token {}.",
        tokens.expires_in.unwrap_or(0),
        if tokens.refresh_token.is_some() {
            "stored in the OS keychain"
        } else {
            "not issued"
        }
    );
    Ok(0)
}

async fn logout(config: &LoamsConfig) -> anyhow::Result<i32> {
    let (client, _mock) = connect(config).await?;
    let instance = client.get_instance().await.context("GetInstance")?;
    auth::sign_out(store().as_ref(), &instance.instance_id)?;
    println!("Signed out of {}.", instance.name);
    Ok(0)
}

fn a2a_client(config: &LoamsConfig) -> Arc<dyn A2aClient> {
    match &config.bot_url {
        Some(url) => Arc::new(JsonRpcA2aClient::new(url.clone(), None)),
        None => {
            if !config.mock {
                eprintln!("LOAMS_BOT_URL is not set; Loams Bot is answering from the mock agent");
            }
            Arc::new(MockA2aClient::default())
        }
    }
}

async fn bot(config: &LoamsConfig, text: &str) -> anyhow::Result<i32> {
    let message = Message {
        message_id: format!("cli-{}", auth::pkce::random_token(6)),
        context_id: None,
        task_id: None,
        role: Role::User,
        parts: vec![Part::text(text)],
    };
    let response = a2a_client(config).send_message(message).await?;
    let reply = Reply::from_response(&response);
    println!("[{:?}] {}", reply.state, reply.text);
    Ok(0)
}

async fn bot_acp(config: &LoamsConfig) -> anyhow::Result<i32> {
    let a2a = a2a_client(config);
    crate::acp::serve(BufReader::new(tokio::io::stdin()), tokio::io::stdout(), a2a).await?;
    Ok(0)
}

async fn mock(addr: &str) -> anyhow::Result<i32> {
    let mock = MockServer::start(
        addr.parse()
            .with_context(|| format!("bad address {addr:?}"))?,
    )
    .await?;
    println!("loams mock listening on {} (Ctrl-C to stop)", mock.url());
    tokio::signal::ctrl_c().await?;
    mock.stop().await;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bot_acp_owns_stdout() {
        assert!(owns_stdout(&["bot-acp".to_owned()]));
        assert!(!owns_stdout(&["status".to_owned()]));
        assert!(!owns_stdout(&[]));
    }

    #[tokio::test]
    async fn unknown_verb_is_a_usage_error() {
        assert_eq!(run(vec!["frobnicate".into()]).await.unwrap(), 2);
        assert!(run(vec!["bot".into()]).await.is_err());
    }

    #[test]
    fn authentik_method_is_found_and_issuer_can_be_overridden() {
        use crate::proto::loams::instance::v1::SignInMethod;
        let instance = GetInstanceResponse {
            sign_in_methods: vec![SignInMethod {
                kind: SignInKind::SIGN_IN_KIND_AUTHENTIK.into(),
                issuer: "https://auth.example/application/o/loams/".into(),
                client_id: "loams-desktop".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let (issuer, client_id) = authentik_issuer(&instance, &LoamsConfig::default()).unwrap();
        assert_eq!(issuer, "https://auth.example/application/o/loams/");
        assert_eq!(client_id, "loams-desktop");
        let dev = LoamsConfig {
            oidc_issuer: Some("http://127.0.0.1:9000/o/".into()),
            ..Default::default()
        };
        assert_eq!(
            authentik_issuer(&instance, &dev).unwrap().0,
            "http://127.0.0.1:9000/o/"
        );
        assert!(authentik_issuer(&GetInstanceResponse::default(), &dev).is_none());
    }
}
