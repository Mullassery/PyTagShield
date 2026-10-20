use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{DeliveryConfig, DestinationConfig};
use crate::consent::ConsentSnapshot;
use crate::destination::{build, HttpRequestPlan, OutboundPlan};
use crate::event::Event;
use crate::metrics::Metrics;

/// One event queued for delivery to one destination, together with the
/// consent snapshot it was processed under.
#[derive(Clone, Debug)]
pub struct DeliveryTask {
    pub event: Event,
    pub consent: ConsentSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Delivered,
    Failed,
}

/// Observable outcome of a delivery (success or exhausted retries).
#[derive(Clone, Debug, Serialize)]
pub struct DeliveryRecord {
    /// Monotonically increasing record id (the ring buffer may evict old
    /// records; ids stay unique).
    pub id: u64,
    pub event_id: String,
    pub event_name: String,
    pub destination: String,
    pub status: DeliveryStatus,
    pub attempts: u32,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub at: DateTime<Utc>,
}

/// Bounded ring buffer of delivery outcomes. Oldest records are evicted
/// first; memory is fixed regardless of traffic.
pub struct DeliveryLog {
    inner: Mutex<VecDeque<DeliveryRecord>>,
    capacity: usize,
    next_id: AtomicU64,
}

impl DeliveryLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
            capacity: capacity.max(1),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn push(&self, mut record: DeliveryRecord) {
        record.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        while guard.len() >= self.capacity {
            guard.pop_front();
        }
        guard.push_back(record);
    }

    /// Most recent records first, up to `limit`.
    pub fn recent(&self, limit: usize) -> Vec<DeliveryRecord> {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.iter().rev().take(limit).cloned().collect()
    }

    pub fn len(&self) -> usize {
        match self.inner.lock() {
            Ok(g) => g.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Outcome classification of one outbound attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptOutcome {
    Success,
    /// Retry within the configured attempt budget (transient failure).
    Retryable,
    /// Do not retry (permanent rejection or budget exhausted).
    Terminal,
}

/// Classify an HTTP status code. Network errors are always retryable.
pub fn classify_status(status: u16) -> AttemptOutcome {
    match status {
        200..=299 => AttemptOutcome::Success,
        408 | 429 => AttemptOutcome::Retryable,
        500..=599 => AttemptOutcome::Retryable,
        _ => AttemptOutcome::Terminal,
    }
}

/// Exponential backoff for the retry following `completed_attempts`
/// (1-based), capped by configuration. Deterministic by design so tests and
/// benchmarks are reproducible.
pub fn backoff_delay(cfg: &DeliveryConfig, completed_attempts: u32) -> Duration {
    let exp = completed_attempts.saturating_sub(1).min(16);
    let ms = cfg
        .initial_backoff_ms
        .saturating_mul(1u64 << exp)
        .min(cfg.max_backoff_ms);
    Duration::from_millis(ms)
}

async fn send(client: &reqwest::Client, plan: &HttpRequestPlan) -> Result<u16, String> {
    let mut request = client.post(&plan.url).json(&plan.body);
    for (k, v) in &plan.headers {
        request = request.header(k.as_str(), v.as_str());
    }
    match request.send().await {
        Ok(response) => Ok(response.status().as_u16()),
        Err(e) => Err(e.without_url().to_string()),
    }
}

/// Consume the bounded queue for one destination, dispatching events with
/// bounded retries and recording every outcome.
pub async fn run_delivery_worker(
    dest: DestinationConfig,
    delivery: DeliveryConfig,
    client: reqwest::Client,
    metrics: std::sync::Arc<Metrics>,
    log: std::sync::Arc<DeliveryLog>,
    mut rx: tokio::sync::mpsc::Receiver<DeliveryTask>,
) {
    tracing::debug!(destination = %dest.name, "delivery worker started");

    while let Some(task) = rx.recv().await {
        let started = std::time::Instant::now();
        let mut attempts = 0u32;
        let mut last_error: Option<String> = None;
        let mut finished = false;

        while !finished {
            attempts += 1;
            Metrics::inc(&metrics.delivery_attempts);

            let plan = match build(&dest, &task.event, &task.consent) {
                Ok(p) => p,
                Err(e) => {
                    last_error = Some(e);
                    break;
                }
            };

            let outcome = match &plan {
                OutboundPlan::Log { .. } => AttemptOutcome::Success,
                OutboundPlan::Http(http) => match send(&client, http).await {
                    Ok(status) => classify_status(status),
                    Err(e) => {
                        last_error = Some(format!("network error: {e}"));
                        AttemptOutcome::Retryable
                    }
                },
            };

            match outcome {
                AttemptOutcome::Success => {
                    finished = true;
                    last_error = None;
                }
                AttemptOutcome::Terminal => {
                    if last_error.is_none() {
                        last_error = Some("destination rejected the event (non-retryable status)".into());
                    }
                    finished = true;
                }
                AttemptOutcome::Retryable => {
                    if attempts >= delivery.max_attempts {
                        if last_error.is_none() {
                            last_error = Some("retry budget exhausted (retryable status)".into());
                        }
                        finished = true;
                    } else {
                        tokio::time::sleep(backoff_delay(&delivery, attempts)).await;
                    }
                }
            }
        }

        let status = if last_error.is_some() {
            DeliveryStatus::Failed
        } else {
            DeliveryStatus::Delivered
        };
        if status == DeliveryStatus::Failed {
            Metrics::inc(&metrics.delivery_failed);
        } else {
            Metrics::inc(&metrics.delivery_succeeded);
        }

        log.push(DeliveryRecord {
            id: 0,
            event_id: task.event.id.clone(),
            event_name: task.event.name.clone(),
            destination: dest.name.clone(),
            status,
            attempts,
            latency_ms: started.elapsed().as_millis() as u64,
            error: last_error,
            at: Utc::now(),
        });
    }

    tracing::debug!(destination = %dest.name, "delivery worker stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_status_codes() {
        assert_eq!(classify_status(200), AttemptOutcome::Success);
        assert_eq!(classify_status(204), AttemptOutcome::Success);
        assert_eq!(classify_status(400), AttemptOutcome::Terminal);
        assert_eq!(classify_status(401), AttemptOutcome::Terminal);
        assert_eq!(classify_status(404), AttemptOutcome::Terminal);
        assert_eq!(classify_status(408), AttemptOutcome::Retryable);
        assert_eq!(classify_status(429), AttemptOutcome::Retryable);
        assert_eq!(classify_status(500), AttemptOutcome::Retryable);
        assert_eq!(classify_status(503), AttemptOutcome::Retryable);
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let cfg = DeliveryConfig {
            initial_backoff_ms: 100,
            max_backoff_ms: 1_000,
            ..DeliveryConfig::default()
        };
        assert_eq!(backoff_delay(&cfg, 1), Duration::from_millis(100));
        assert_eq!(backoff_delay(&cfg, 2), Duration::from_millis(200));
        assert_eq!(backoff_delay(&cfg, 3), Duration::from_millis(400));
        assert_eq!(backoff_delay(&cfg, 4), Duration::from_millis(800));
        assert_eq!(backoff_delay(&cfg, 5), Duration::from_millis(1_000));
        assert_eq!(backoff_delay(&cfg, 50), Duration::from_millis(1_000));
    }

    #[test]
    fn delivery_log_is_bounded_and_newest_first() {
        let log = DeliveryLog::new(3);
        for i in 0..5 {
            log.push(DeliveryRecord {
                id: 0,
                event_id: format!("e{i}"),
                event_name: "n".into(),
                destination: "d".into(),
                status: DeliveryStatus::Delivered,
                attempts: 1,
                latency_ms: 0,
                error: None,
                at: Utc::now(),
            });
        }
        assert_eq!(log.len(), 3);
        let recent = log.recent(10);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].event_id, "e4");
        assert_eq!(recent[2].event_id, "e2");
        // Ids are unique and increasing even after eviction.
        assert!(recent[0].id > recent[1].id && recent[1].id > recent[2].id);
    }
}
