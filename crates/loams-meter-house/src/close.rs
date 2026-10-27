//! The close job (design §63 §8.2; ME1 Task 6): every 60 s the `meter` role
//! closes each meter's minute windows that ended at least 2 minutes ago,
//! re-closes the windows late events fell into, and rolls minutes up into
//! hours and days (or recomputes those from raw events for `UNIQUE_COUNT`,
//! `AVG` and `LATEST`).
//!
//! The work is [`sql::CloseRun`], a sans-I/O machine; this module drives it
//! against a [`HouseSql`]. A run is idempotent and safe on every meter
//! instance at once: window rows are keyed and versioned (`close_run`), and
//! a run recomputes whole windows, so racing runs leave the same rows.

use crate::house::{HouseError, HouseSql};
use crate::sql::{CloseConfig, CloseRun, CloseSummary, MeterDef};

/// Why a close run stopped.
#[derive(Debug, thiserror::Error)]
pub enum CloseError {
    #[error(transparent)]
    House(#[from] HouseError),
    /// The House answered a close-job query with something unreadable.
    #[error("meter {meter}: {message}")]
    Answer { meter: String, message: String },
}

/// Drives one sans-I/O machine to its end.
pub(crate) async fn drive<H: HouseSql>(
    house: &H,
    meter: &str,
    mut next: impl FnMut(Option<&str>) -> Result<Option<String>, String>,
) -> Result<(), CloseError> {
    let mut answer: Option<String> = None;
    loop {
        let step = next(answer.as_deref()).map_err(|message| CloseError::Answer {
            meter: meter.to_string(),
            message,
        })?;
        let Some(statement) = step else {
            return Ok(());
        };
        answer = Some(house.query(&statement).await?);
    }
}

/// The close job of §63 §8.2.
#[derive(Debug, Clone, Copy, Default)]
pub struct CloseJob {
    pub config: CloseConfig,
}

impl CloseJob {
    /// One close run of `meter` at `now_ms` (Unix milliseconds; also the
    /// run's version).
    pub async fn close_meter<H: HouseSql>(
        &self,
        house: &H,
        meter: &MeterDef,
        now_ms: i64,
    ) -> Result<CloseSummary, CloseError> {
        let mut run = CloseRun::new(meter.clone(), now_ms, self.config);
        drive(house, &meter.slug, |answer| run.next(answer)).await?;
        Ok(run.summary().clone())
    }

    /// One run over every meter. A meter that fails does not stop the
    /// others; its error is returned beside the others' summaries, and the
    /// next run retries it (nothing was recorded for it).
    pub async fn run_once<H: HouseSql>(
        &self,
        house: &H,
        meters: &[MeterDef],
        now_ms: i64,
    ) -> Vec<(String, Result<CloseSummary, CloseError>)> {
        let mut out = Vec::with_capacity(meters.len());
        for meter in meters {
            let result = self.close_meter(house, meter, now_ms).await;
            out.push((meter.slug.clone(), result));
        }
        out
    }
}
