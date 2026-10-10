//! The link CLI's one verb. `loams-agentd` exposes it as `loams-agentd loams
//! bot-acp` (plan DD1 ruling T1-2): the Loams Bot harness launches it to
//! serve Loams Bot to the sessions engine over ACP (stdio). The fork's other
//! verbs (`status`, `login`, `logout`, `bot`, `mock`) were removed in DD1
//! Task 3: they signed in, reached the instance or bound a port, and nothing
//! could call them.
//!
//! `LOAMS_BOT_URL` and `LOAMS_MOCK` configure it (see [`crate::config`]).

use std::sync::Arc;

use tokio::io::BufReader;

use crate::a2a::{A2aClient, JsonRpcA2aClient, MockA2aClient};
use crate::config::LoamsConfig;

/// Usage text for the link CLI with no or unknown arguments.
pub const USAGE: &str = "usage: bot-acp";

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
    match args.first().map(String::as_str) {
        Some("bot-acp") => bot_acp(&LoamsConfig::from_env()).await,
        _ => {
            eprintln!("{USAGE}");
            Ok(2)
        }
    }
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

async fn bot_acp(config: &LoamsConfig) -> anyhow::Result<i32> {
    let a2a = a2a_client(config);
    crate::acp::serve(BufReader::new(tokio::io::stdin()), tokio::io::stdout(), a2a).await?;
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
    async fn every_other_verb_is_a_usage_error() {
        for verb in ["status", "login", "logout", "bot", "mock", "frobnicate"] {
            assert_eq!(run(vec![verb.into()]).await.unwrap(), 2, "{verb}");
        }
        assert_eq!(run(Vec::new()).await.unwrap(), 2);
    }
}
