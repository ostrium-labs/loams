//! Per-thread usage (design §53 §6.5 G20; plan SD1 Task 14).
//!
//! [`ThreadUsageLog`] follows each chat's turns as a [`TurnObserver`]: it adds
//! up the harness's `Usage` events between a turn's start and its end, and
//! when the turn ends writes one `turn_usage` row (tokens, the harness's own
//! cost if it reported one, and the duration). `GetThreadUsage` reads them
//! back. It is a display for the thread header, informational only: there
//! is no price table and nothing here has a billing meaning (billing-grade
//! metering is a separate design, §63).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use loams_agentd_proto::AgentEvent;
use loams_agentd_proto::usage::{ThreadUsage, TurnUsage};
use loams_agentd_store::{DocsStore, StoreError, TurnUsageRow};

use crate::sessions::TurnObserver;

/// The turn of a chat in progress.
#[derive(Debug)]
struct OpenTurn {
    turn_id: String,
    started: Instant,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_tokens: Option<u64>,
    cost_usd: Option<f64>,
}

fn add(sum: Option<u64>, value: Option<u64>) -> Option<u64> {
    match (sum, value) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, b) => a.or(b),
    }
}

impl OpenTurn {
    fn new(turn_id: &str) -> Self {
        Self {
            turn_id: turn_id.to_owned(),
            started: Instant::now(),
            input_tokens: None,
            output_tokens: None,
            cache_tokens: None,
            cost_usd: None,
        }
    }

    fn row(&self) -> TurnUsageRow {
        TurnUsageRow {
            turn_id: self.turn_id.clone(),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_tokens: self.cache_tokens,
            cost_usd: self.cost_usd,
            duration_ms: i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX),
        }
    }
}

/// Each chat's turns' usage: the open turns in memory, the ended ones in the
/// store.
#[derive(Clone)]
pub struct ThreadUsageLog {
    store: Arc<DocsStore>,
    open: Arc<Mutex<HashMap<String, OpenTurn>>>,
}

impl std::fmt::Debug for ThreadUsageLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadUsageLog").finish_non_exhaustive()
    }
}

impl ThreadUsageLog {
    pub fn new(store: Arc<DocsStore>) -> Self {
        Self {
            store,
            open: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn open(&self) -> MutexGuard<'_, HashMap<String, OpenTurn>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, chat_id: &str, turn: &OpenTurn) {
        if let Err(error) = self.store.record_turn_usage(chat_id, &turn.row()) {
            tracing::warn!(chat = %chat_id, %error, "turn usage write failed");
        }
    }

    /// A chat's ended turns, oldest first, with their totals.
    ///
    /// # Errors
    ///
    /// The store's.
    pub fn thread(&self, chat_id: &str) -> Result<ThreadUsage, StoreError> {
        let turns = self
            .store
            .turn_usage(chat_id)?
            .into_iter()
            .map(|row| TurnUsage {
                turn_id: row.turn_id,
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cache_tokens: row.cache_tokens,
                cost_usd: row.cost_usd,
                duration_ms: row.duration_ms,
            })
            .collect();
        Ok(ThreadUsage::new(turns))
    }

    /// Forget a deleted chat's usage.
    pub fn forget(&self, chat_id: &str) {
        self.open().remove(chat_id);
        if let Err(error) = self.store.delete_turn_usage(chat_id) {
            tracing::warn!(chat = %chat_id, %error, "turn usage delete failed");
        }
    }
}

impl TurnObserver for ThreadUsageLog {
    fn turn_start(&self, chat_id: &str, message_id: &str) {
        // A steer starts a turn of its own: the one it interrupts ends here.
        let previous = self
            .open()
            .insert(chat_id.to_owned(), OpenTurn::new(message_id));
        if let Some(previous) = previous
            && previous.turn_id != message_id
        {
            self.record(chat_id, &previous);
        }
    }

    fn event(&self, chat_id: &str, event: &AgentEvent) {
        // A subagent's usage (`AgentEvent::Subagent`) is not the thread's own
        // turn; the harnesses report the parent turn's total themselves.
        let AgentEvent::Usage {
            input_tokens,
            output_tokens,
            cache_tokens,
            cost_usd,
        } = event
        else {
            return;
        };
        if let Some(turn) = self.open().get_mut(chat_id) {
            turn.input_tokens = add(turn.input_tokens, Some(*input_tokens));
            turn.output_tokens = add(turn.output_tokens, Some(*output_tokens));
            turn.cache_tokens = add(turn.cache_tokens, *cache_tokens);
            turn.cost_usd = match (turn.cost_usd, *cost_usd) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            };
        }
    }

    fn turn_end(&self, chat_id: &str) {
        let turn = self.open().remove(chat_id);
        if let Some(turn) = turn {
            self.record(chat_id, &turn);
        }
    }
}
