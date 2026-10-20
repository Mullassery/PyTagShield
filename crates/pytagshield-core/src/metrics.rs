use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide counters exposed in Prometheus text format.
///
/// All counters are monotonic `u64` atomics: no locks on the hot path and
/// bounded memory regardless of traffic.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Ingest payloads received (single events; batch items counted once
    /// per item).
    pub events_received: AtomicU64,
    /// Events rejected during validation.
    pub events_rejected: AtomicU64,
    /// Events that completed the pipeline.
    pub events_accepted: AtomicU64,
    /// Accepted events that matched no route.
    pub events_unrouted: AtomicU64,
    /// Route evaluations that matched at least one route.
    pub routes_matched: AtomicU64,
    /// Destination selections blocked by consent.
    pub consent_denied_destinations: AtomicU64,
    /// Destination selections blocked because the destination is disabled.
    pub disabled_destinations: AtomicU64,
    /// Successful enqueue operations.
    pub enqueued: AtomicU64,
    /// Enqueue rejections because a bounded queue was full (backpressure).
    pub enqueue_rejected: AtomicU64,
    /// Properties removed by minimization.
    pub properties_dropped: AtomicU64,
    /// String values truncated by minimization.
    pub values_truncated: AtomicU64,
    /// Events whose distinct id was pseudonymized.
    pub ids_pseudonymized: AtomicU64,
    /// Enrichment actions skipped because consent granted nothing.
    pub enrichment_skipped: AtomicU64,
    /// Delivery attempts (every retry counts).
    pub delivery_attempts: AtomicU64,
    /// Deliveries that ultimately succeeded.
    pub delivery_succeeded: AtomicU64,
    /// Deliveries abandoned after the configured attempt budget.
    pub delivery_failed: AtomicU64,
    /// HTTP requests rejected by the concurrency limiter or auth layer.
    pub requests_denied: AtomicU64,
}

impl Metrics {
    pub fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    /// Render counters in Prometheus text exposition format.
    pub fn render_prometheus(&self) -> String {
        fn line(out: &mut String, name: &str, help: &str, value: u64) {
            out.push_str(&format!("# HELP {name} {help}\n"));
            out.push_str(&format!("# TYPE {name} counter\n"));
            out.push_str(&format!("{name} {value}\n"));
        }

        let mut out = String::with_capacity(4096);
        line(&mut out, "pytagshield_events_received", "Ingest events received", self.events_received.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_events_rejected", "Events rejected by validation", self.events_rejected.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_events_accepted", "Events accepted by the pipeline", self.events_accepted.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_events_unrouted", "Accepted events matching no route", self.events_unrouted.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_routes_matched", "Events matching at least one route", self.routes_matched.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_consent_denied_destinations", "Destination selections denied by consent", self.consent_denied_destinations.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_disabled_destinations", "Destination selections skipped (disabled)", self.disabled_destinations.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_enqueued", "Events enqueued for delivery", self.enqueued.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_enqueue_rejected", "Enqueue rejections due to full queues", self.enqueue_rejected.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_properties_dropped", "Properties dropped by minimization", self.properties_dropped.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_values_truncated", "Values truncated by minimization", self.values_truncated.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_ids_pseudonymized", "Distinct ids pseudonymized", self.ids_pseudonymized.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_enrichment_skipped", "Enrichment actions skipped by consent", self.enrichment_skipped.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_delivery_attempts", "Outbound delivery attempts", self.delivery_attempts.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_delivery_succeeded", "Successful outbound deliveries", self.delivery_succeeded.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_delivery_failed", "Deliveries abandoned after retries", self.delivery_failed.load(Ordering::Relaxed));
        line(&mut out, "pytagshield_requests_denied", "Requests denied by auth or limits", self.requests_denied.load(Ordering::Relaxed));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment_and_render() {
        let m = Metrics::default();
        Metrics::inc(&m.events_received);
        Metrics::add(&m.properties_dropped, 3);
        assert_eq!(m.events_received.load(Ordering::Relaxed), 1);
        assert_eq!(m.properties_dropped.load(Ordering::Relaxed), 3);

        let text = m.render_prometheus();
        assert!(text.contains("pytagshield_events_received 1"));
        assert!(text.contains("pytagshield_properties_dropped 3"));
        assert!(text.contains("# TYPE pytagshield_events_received counter"));
        // HELP, TYPE and sample line each mention the metric name once.
        assert_eq!(text.matches("pytagshield_events_rejected").count(), 3);
    }
}
