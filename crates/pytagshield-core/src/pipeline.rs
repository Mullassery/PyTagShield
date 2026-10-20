use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;
use crate::attribution::{preserve as preserve_attribution, AttributionReport};
use crate::consent::{resolve as resolve_consent, ConsentSnapshot, ConsentStore};
use crate::error::PipelineError;
use crate::event::{Event, EventContext};
use crate::metrics::Metrics;
use crate::minimize::{minimize, MinimizationReport};
use crate::route::{resolve as resolve_routes, RoutingOutcome, SkippedDestination};
use crate::transform::{apply as apply_transforms, TransformReport};

/// Why a selected destination did not receive an enqueue attempt.
#[derive(Clone, Debug, Serialize)]
pub struct EnqueueRejection {
    pub destination: String,
    pub reason: String,
}

/// The complete, observable result of processing one ingest payload.
#[derive(Clone, Debug, Serialize)]
pub struct ProcessedEvent {
    pub event: Event,
    pub consent: ConsentSnapshot,
    pub routing: RoutingOutcome,
    pub transforms: TransformReport,
    pub attribution: AttributionReport,
    pub minimization: MinimizationReport,
}

/// Response-shaped summary of one ingest item, including enqueue results
/// filled in by the transport layer.
#[derive(Clone, Debug, Serialize)]
pub struct IngestOutcome {
    pub event_id: String,
    pub event_name: String,
    pub status: &'static str,
    pub consent: BTreeMap<String, bool>,
    pub consent_source: crate::consent::ConsentSource,
    pub matched_routes: Vec<String>,
    pub destinations: Vec<String>,
    pub enqueued: Vec<String>,
    pub rejected: Vec<EnqueueRejection>,
    pub skipped: Vec<SkippedDestination>,
    pub transforms: TransformReport,
    pub attribution: AttributionReport,
    pub minimization: MinimizationReport,
}

impl IngestOutcome {
    /// Build the outcome shell for a processed event; the transport layer
    /// then records enqueue results per destination.
    pub fn new(processed: ProcessedEvent) -> Self {
        Self {
            event_id: processed.event.id.clone(),
            event_name: processed.event.name.clone(),
            status: "accepted",
            consent: processed.consent.granted(),
            consent_source: processed.consent.source,
            matched_routes: processed.routing.matched_routes,
            destinations: processed.routing.destinations,
            enqueued: Vec::new(),
            rejected: Vec::new(),
            skipped: processed.routing.skipped,
            transforms: processed.transforms,
            attribution: processed.attribution,
            minimization: processed.minimization,
        }
    }

    /// Record an enqueue rejection and downgrade status accordingly.
    pub fn reject(&mut self, destination: &str, reason: &str) {
        self.rejected.push(EnqueueRejection {
            destination: destination.to_owned(),
            reason: reason.to_owned(),
        });
        self.status = if !self.enqueued.is_empty() {
            "partially_accepted"
        } else if self.rejected.iter().all(|r| r.reason == "queue_full") {
            "backpressure"
        } else {
            "rejected"
        };
    }

    pub fn record_enqueue(&mut self, destination: &str) {
        self.enqueued.push(destination.to_owned());
    }
}

/// The synchronous, side-effect-free portion of the tagging pipeline:
/// validate -> normalize -> consent -> transform -> minimize -> route.
///
/// Network I/O happens later, in the bounded delivery workers.
pub struct Pipeline {
    config: Arc<Config>,
    consent_store: Arc<ConsentStore>,
    metrics: Arc<Metrics>,
}

impl Pipeline {
    pub fn new(config: Arc<Config>, consent_store: Arc<ConsentStore>, metrics: Arc<Metrics>) -> Self {
        Self {
            config,
            consent_store,
            metrics,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn consent_store(&self) -> &ConsentStore {
        &self.consent_store
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Process one raw ingest payload through every pipeline stage.
    ///
    /// The returned event is fully minimized and routed; enqueueing to
    /// destination queues is the transport layer's responsibility so that
    /// backpressure stays explicit at the HTTP boundary.
    pub fn process(
        &self,
        raw: &Value,
        context: EventContext,
    ) -> Result<ProcessedEvent, PipelineError> {
        Metrics::inc(&self.metrics.events_received);

        let now = Utc::now();
        let mut event = match Event::parse(raw, &self.config.ingest, now) {
            Ok(mut e) => {
                e.context = context;
                e
            }
            Err(err) => {
                Metrics::inc(&self.metrics.events_rejected);
                return Err(PipelineError::Validation(err));
            }
        };

        // Consent resolution uses the raw identifier, which stays in memory
        // only and is pseudonymized later, before any egress.
        let stored = self
            .consent_store
            .get(&event.distinct_id, now, self.config.consent.store_ttl_secs);
        let consent = resolve_consent(event.consent.as_ref(), stored.as_ref(), &self.config.consent);

        let mut transform_report = TransformReport::default();
        apply_transforms(&mut event, &self.config.transforms, &consent, &mut transform_report);

        let mut attribution_report = AttributionReport::default();
        preserve_attribution(
            &mut event,
            &self.config.attribution,
            &consent,
            &mut attribution_report,
        );

        let mut minimization_report = MinimizationReport::default();
        minimize(&mut event, &self.config.minimization, &mut minimization_report);

        let routing = resolve_routes(
            &event,
            &self.config.routes,
            &self.config.destinations,
            &consent,
        );

        // Metrics bookkeeping.
        Metrics::inc(&self.metrics.events_accepted);
        if !routing.matched_routes.is_empty() {
            Metrics::inc(&self.metrics.routes_matched);
        }
        if routing.matched_routes.is_empty() {
            Metrics::inc(&self.metrics.events_unrouted);
        }
        for skipped in &routing.skipped {
            if skipped.reason.starts_with("consent_denied") {
                Metrics::inc(&self.metrics.consent_denied_destinations);
            } else if skipped.reason == "destination_disabled" {
                Metrics::inc(&self.metrics.disabled_destinations);
            }
        }
        Metrics::add(
            &self.metrics.properties_dropped,
            minimization_report.properties_dropped as u64,
        );
        Metrics::add(
            &self.metrics.values_truncated,
            minimization_report.values_truncated as u64,
        );
        if minimization_report.id_pseudonymized {
            Metrics::inc(&self.metrics.ids_pseudonymized);
        }
        Metrics::add(
            &self.metrics.enrichment_skipped,
            transform_report.enrichment_skipped as u64,
        );

        Ok(ProcessedEvent {
            event,
            consent,
            routing,
            transforms: transform_report,
            attribution: attribution_report,
            minimization: minimization_report,
        })
    }
}

/// Convenience wrapper: process an event and enqueue it to every routed
/// destination, recording backpressure explicitly.
///
/// Returns one outcome per ingest payload. Transport layers with their own
/// queue plumbing can call [`Pipeline::process`] directly.
pub fn process_and_enqueue(
    pipeline: &Pipeline,
    queues: &std::collections::HashMap<String, tokio::sync::mpsc::Sender<crate::delivery::DeliveryTask>>,
    raw: &Value,
    context: EventContext,
) -> Result<IngestOutcome, PipelineError> {
    let processed = pipeline.process(raw, context)?;
    let event = processed.event.clone();
    let consent = processed.consent.clone();
    let mut outcome = IngestOutcome::new(processed);

    for destination in outcome.destinations.clone() {
        let Some(tx) = queues.get(&destination) else {
            outcome.reject(&destination, "no_queue_for_destination");
            continue;
        };
        let task = crate::delivery::DeliveryTask {
            event: event.clone(),
            consent: consent.clone(),
        };
        match tx.try_send(task) {
            Ok(()) => {
                Metrics::inc(&pipeline.metrics().enqueued);
                outcome.record_enqueue(&destination);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                Metrics::inc(&pipeline.metrics().enqueue_rejected);
                outcome.reject(&destination, "queue_full");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                outcome.reject(&destination, "queue_closed");
            }
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc;

    fn config(yaml: &str) -> Arc<Config> {
        Arc::new(Config::from_yaml(yaml).expect("valid test config"))
    }

    fn base_config(extra: &str) -> String {
        format!(
            r#"
minimization:
  pseudonymize_salt: "unit-test-salt"
destinations:
  - name: ph
    type: posthog
    purpose: analytics
    posthog:
      api_key: "phc_test"
      host: "https://us.i.posthog.com"
  - name: ads
    type: log
    purpose: advertising
routes:
  - name: all
    destinations: [ph, ads]
{extra}
"#
        )
    }

    fn pipeline(yaml: &str) -> (Pipeline, Arc<Metrics>) {
        let metrics = Arc::new(Metrics::default());
        let p = Pipeline::new(
            config(yaml),
            Arc::new(ConsentStore::new()),
            metrics.clone(),
        );
        (p, metrics)
    }

    /// Test queues: keeps both senders (for enqueue) and receivers (alive so
    /// channels stay open, and readable to inspect what was queued).
    struct TestQueues {
        senders: std::collections::HashMap<String, mpsc::Sender<crate::delivery::DeliveryTask>>,
        receivers: std::collections::HashMap<String, mpsc::Receiver<crate::delivery::DeliveryTask>>,
    }

    impl TestQueues {
        fn new(names: &[&str], cap: usize) -> Self {
            let mut senders = std::collections::HashMap::new();
            let mut receivers = std::collections::HashMap::new();
            for n in names {
                let (tx, rx) = mpsc::channel(cap);
                senders.insert(n.to_string(), tx);
                receivers.insert(n.to_string(), rx);
            }
            Self { senders, receivers }
        }
    }

    fn raw_event(consent: Option<serde_json::Value>) -> Value {
        let mut e = serde_json::json!({
            "event": "page_view",
            "distinct_id": "user-42",
            "properties": {"email": "a@b.c", "url": "/home", "debug": true},
        });
        if let Some(c) = consent {
            e["consent"] = c;
        }
        e
    }

    #[test]
    fn without_consent_nothing_routes_even_if_accepted() {
        let (p, _metrics) = pipeline(&base_config(""));
        let q = TestQueues::new(&["ph", "ads"], 4);

        let outcome = process_and_enqueue(&p, &q.senders, &raw_event(None), EventContext::default()).unwrap();
        assert_accepted_without_consent(&outcome);
    }

    fn assert_accepted_without_consent(outcome: &IngestOutcome) {
        // Default policy is deny: without explicit consent nothing routes.
        assert!(outcome.destinations.is_empty());
        assert!(outcome.enqueued.is_empty());
        assert_eq!(outcome.skipped.len(), 2);
        assert!(outcome
            .skipped
            .iter()
            .all(|s| s.reason.starts_with("consent_denied")));
        // The event itself still completed the pipeline.
        assert_eq!(outcome.status, "accepted");
    }

    #[test]
    fn granted_consent_routes_and_minimizes() {
        let (p, metrics) = pipeline(&base_config(""));
        let mut q = TestQueues::new(&["ph", "ads"], 4);
        let consent = serde_json::json!({"analytics": true, "advertising": true});

        let outcome =
            process_and_enqueue(&p, &q.senders, &raw_event(Some(consent)), EventContext::default()).unwrap();

        assert_eq!(outcome.status, "accepted");
        assert_eq!(outcome.destinations, vec!["ph", "ads"]);
        assert_eq!(outcome.enqueued, vec!["ph", "ads"]);
        assert_eq!(outcome.minimization.properties_dropped, 1); // email
        assert!(outcome.minimization.id_pseudonymized);

        // Queue received a fully minimized event.
        let task = q.receivers.get_mut("ph").unwrap().try_recv().unwrap();
        assert!(task.event.distinct_id.starts_with("sha256:"));
        assert_ne!(task.event.distinct_id, "user-42");
        assert!(!task.event.properties.contains_key("email"));
        assert!(task.event.properties.contains_key("url"));

        assert_eq!(metrics.events_accepted.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.enqueued.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.properties_dropped.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.ids_pseudonymized.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn partial_consent_blocks_advertising_destination() {
        let (p, _metrics) = pipeline(&base_config(""));
        let q = TestQueues::new(&["ph", "ads"], 4);
        let consent = serde_json::json!({"analytics": true, "advertising": false});

        let outcome =
            process_and_enqueue(&p, &q.senders, &raw_event(Some(consent)), EventContext::default()).unwrap();

        assert_eq!(outcome.destinations, vec!["ph"]);
        assert_eq!(outcome.enqueued, vec!["ph"]);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].reason, "consent_denied:advertising");
    }

    #[test]
    fn validation_failure_is_rejected_and_counted() {
        let (p, metrics) = pipeline(&base_config(""));
        let q = TestQueues::new(&["ph", "ads"], 4);
        let err = process_and_enqueue(
            &p,
            &q.senders,
            &serde_json::json!({"properties": {}}),
            EventContext::default(),
        )
        .expect_err("missing event/distinct_id must fail");
        assert!(err.is_validation());
        assert_eq!(metrics.events_rejected.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.events_accepted.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn full_queue_yields_explicit_backpressure() {
        let (p, _metrics) = pipeline(&base_config(""));
        // Capacity 1 and no consumer: first enqueue fills the buffer.
        let q = TestQueues::new(&["ph", "ads"], 1);
        let consent = serde_json::json!({"analytics": true, "advertising": true});

        let first =
            process_and_enqueue(&p, &q.senders, &raw_event(Some(consent.clone())), EventContext::default())
                .unwrap();
        assert_eq!(first.status, "accepted");

        let second =
            process_and_enqueue(&p, &q.senders, &raw_event(Some(consent.clone())), EventContext::default())
                .unwrap();
        assert_eq!(second.status, "backpressure");
        assert!(second
            .rejected
            .iter()
            .all(|r| r.reason == "queue_full"));

        // Backpressure must not become silent loss: counters reflect it.
        assert_eq!(p.metrics().enqueue_rejected.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn enrichment_never_runs_without_any_granted_purpose() {
        let yaml = base_config(
            r#"transforms:
  - actions:
      - action: set
        key: enriched
        value: true
      - action: drop
        keys: [debug]
"#,
        );
        let (p, _m) = pipeline(&yaml);
        let q = TestQueues::new(&["ph", "ads"], 4);

        let outcome =
            process_and_enqueue(&p, &q.senders, &raw_event(None), EventContext::default()).unwrap();
        assert_eq!(outcome.transforms.enrichment_skipped, 1);
        assert_eq!(outcome.transforms.actions_applied, 1); // drop still runs
        assert_eq!(outcome.transforms.rules_applied, 1);
    }

    #[test]
    fn no_matching_route_reports_unrouted_without_error() {
        let yaml = r#"
minimization:
  pseudonymize_salt: "s"
destinations:
  - name: ph
    type: posthog
    posthog:
      api_key: k
routes:
  - name: only-purchase
    destinations: [ph]
    match:
      events: [purchase]
"#;
        let (p, metrics) = pipeline(yaml);
        let q = TestQueues::new(&["ph"], 4);
        let outcome = process_and_enqueue(
            &p,
            &q.senders,
            &serde_json::json!({"event":"page_view","distinct_id":"u"}),
            EventContext::default(),
        )
        .unwrap();
        assert_eq!(outcome.status, "accepted");
        assert!(outcome.destinations.is_empty());
        assert!(outcome.matched_routes.is_empty());
        assert_eq!(metrics.events_unrouted.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn attribution_preserves_click_ids_end_to_end() {
        let yaml = base_config(
            r#"attribution:
  preserve_click_ids: true
"#,
        );
        let (p, _metrics) = pipeline(&yaml);
        let mut q = TestQueues::new(&["ph", "ads"], 4);
        let consent = serde_json::json!({"analytics": true, "advertising": true});
        let raw = serde_json::json!({
            "event": "purchase",
            "distinct_id": "user-42",
            "properties": {"gclid": "g-123", "url": "/checkout", "email": "a@b.c"},
            "consent": consent,
        });

        let outcome =
            process_and_enqueue(&p, &q.senders, &raw, EventContext::default()).unwrap();
        assert_eq!(outcome.attribution.click_ids_preserved, 1);
        assert!(outcome.attribution.preserved.contains(&"gclid".to_string()));

        // The preserved click ID survives minimization and reaches the
        // destination as part of $attribution.
        let task = q.receivers.get_mut("ph").unwrap().try_recv().unwrap();
        assert_eq!(
            task.event.properties["$attribution"]["gclid"],
            "g-123"
        );
        // PII is still dropped independently.
        assert!(!task.event.properties.contains_key("email"));
    }
}
