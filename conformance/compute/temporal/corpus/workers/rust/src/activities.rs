//! The corpus's Rust activities, named as the Go and TypeScript ones
//! (`greet`, `heartbeat_thrice`, `fail_twice`, `sleep_for`, `large`), so the
//! histories' activity types match the other languages'.

use std::time::Duration;

use temporalio_macros::activities;
use temporalio_sdk::{
    ApplicationFailure,
    activities::{ActivityContext, ActivityError},
};

pub struct CorpusActivities;

#[activities]
impl CorpusActivities {
    #[activity(name = "greet")]
    pub async fn greet(_ctx: ActivityContext, name: String) -> Result<String, ActivityError> {
        Ok(greeting(&name))
    }

    /// Heartbeats 1, 2 and 3 (the SDK may throttle and send fewer), then
    /// returns 3.
    #[activity(name = "heartbeat_thrice")]
    pub async fn heartbeat_thrice(ctx: ActivityContext) -> Result<i64, ActivityError> {
        for i in 1..=3i64 {
            ctx.record_heartbeat(i).await?;
        }
        Ok(3)
    }

    /// Fails attempts 1 and 2 with a retryable `Transient` error; attempt 3
    /// returns its number.
    #[activity(name = "fail_twice")]
    pub async fn fail_twice(ctx: ActivityContext) -> Result<i64, ActivityError> {
        let attempt = ctx.info().attempt;
        if attempt < 3 {
            return Err(ActivityError::application(
                ApplicationFailure::builder(format!("attempt {attempt} fails"))
                    .type_name("Transient".to_owned())
                    .build(),
            ));
        }
        Ok(i64::from(attempt))
    }

    /// Sleeps `ms` milliseconds without heartbeating; stops early when the
    /// activity is cancelled (a timed-out attempt is cancelled by the SDK).
    #[activity(name = "sleep_for")]
    pub async fn sleep_for(ctx: ActivityContext, ms: u64) -> Result<(), ActivityError> {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
            _ = ctx.cancelled() => Err(ActivityError::cancelled()),
        }
    }

    /// `n` bytes of `x` (large_payload_1mb asks for 1 MiB).
    #[activity(name = "large")]
    pub async fn large(_ctx: ActivityContext, n: u64) -> Result<String, ActivityError> {
        Ok(filler(n))
    }
}

pub fn greeting(name: &str) -> String {
    format!("hello {name}")
}

pub fn filler(n: u64) -> String {
    "x".repeat(usize::try_from(n).unwrap_or(usize::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_matches_the_other_languages() {
        assert_eq!(greeting("corpus"), "hello corpus");
    }

    #[test]
    fn filler_is_n_bytes() {
        assert_eq!(filler(0), "");
        assert_eq!(filler(1 << 20).len(), 1 << 20);
    }
}
