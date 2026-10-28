//! The gateway's own counters (plan KV2 Tasks 10 and 13), rendered in the
//! Prometheus text format. No metric carries a tenant label (D1021).

use std::sync::atomic::{AtomicU64, Ordering};

/// The process's `loams-resp` counters.
#[derive(Debug)]
pub struct RespMetrics {
    /// Keys the expiry worker deleted (`loams_resp_expired_total`).
    pub expired: AtomicU64,
    /// Data keys the GC worker deleted (`loams_resp_gc_keys_total`).
    pub gc_keys: AtomicU64,
}

static METRICS: RespMetrics = RespMetrics {
    expired: AtomicU64::new(0),
    gc_keys: AtomicU64::new(0),
};

/// The process's counters.
pub fn metrics() -> &'static RespMetrics {
    &METRICS
}

impl RespMetrics {
    /// The counters in the Prometheus text format.
    pub fn render(&self) -> String {
        format!(
            "# HELP loams_resp_expired_total Keys deleted by the expiry worker.\n\
             # TYPE loams_resp_expired_total counter\n\
             loams_resp_expired_total {}\n\
             # HELP loams_resp_gc_keys_total Data keys deleted by the GC worker.\n\
             # TYPE loams_resp_gc_keys_total counter\n\
             loams_resp_gc_keys_total {}\n",
            self.expired.load(Ordering::Relaxed),
            self.gc_keys.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_have_no_tenant_label() {
        let text = metrics().render();
        assert!(text.contains("loams_resp_expired_total "));
        assert!(text.contains("loams_resp_gc_keys_total "));
        assert!(!text.contains('{'), "no labels at all: {text}");
    }
}
