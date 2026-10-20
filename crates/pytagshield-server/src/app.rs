//! Application state, routing and bootstrap for the PyTagShield server.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use tokio::sync::mpsc;
use tower::limit::ConcurrencyLimitLayer;

use pytagshield_core::{
    Config, ConsentStore, DeliveryLog, DeliveryTask, Metrics, Pipeline,
};

use crate::handlers;

/// Shared, immutable-after-startup server state.
pub struct AppState {
    pub config: Arc<Config>,
    pub pipeline: Arc<Pipeline>,
    pub metrics: Arc<Metrics>,
    pub delivery_log: Arc<DeliveryLog>,
    /// Bounded per-destination ingress queues (enabled destinations only).
    pub queues: HashMap<String, mpsc::Sender<DeliveryTask>>,
    pub started_at: chrono::DateTime<Utc>,
}

impl AppState {
    pub fn uptime_secs(&self) -> i64 {
        Utc::now().signed_duration_since(self.started_at).num_seconds()
    }
}

/// Build state and spawn one delivery worker per enabled destination.
///
/// Must run inside a Tokio runtime (the delivery workers are spawned onto
/// the current runtime).
pub fn build_state(config: Config) -> Arc<AppState> {
    let config = Arc::new(config);
    let metrics = Arc::new(Metrics::default());
    let consent_store = Arc::new(ConsentStore::new());
    let delivery_log = Arc::new(DeliveryLog::new(config.delivery.log_capacity));
    let pipeline = Arc::new(Pipeline::new(
        config.clone(),
        consent_store,
        metrics.clone(),
    ));

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.delivery.http_timeout_secs))
        .connect_timeout(Duration::from_secs(3))
        .build()
        .expect("reqwest client");

    let mut queues = HashMap::new();
    for dest in &config.destinations {
        if !dest.enabled {
            continue;
        }
        let capacity = dest.effective_queue_capacity(&config.delivery);
        let (tx, rx) = mpsc::channel(capacity);
        queues.insert(dest.name.clone(), tx);
        tokio::spawn(pytagshield_core::delivery::run_delivery_worker(
            dest.clone(),
            config.delivery.clone(),
            http.clone(),
            metrics.clone(),
            delivery_log.clone(),
            rx,
        ));
    }

    tracing::info!(
        destinations = queues.len(),
        routes = config.routes.len(),
        listen = %config.server.listen,
        "pytagshield state built"
    );

    Arc::new(AppState {
        config,
        pipeline,
        metrics,
        delivery_log,
        queues,
        started_at: Utc::now(),
    })
}

/// Build the HTTP router for a state value.
pub fn build_router(state: Arc<AppState>) -> Router {
    let body_limit = state.config.server.max_body_bytes;
    let concurrency = state.config.server.max_concurrent_requests;

    let authenticated = Router::new()
        .route("/api/v1/event", post(handlers::ingest_event))
        .route("/api/v1/batch", post(handlers::ingest_batch))
        .route("/api/v1/consent", post(handlers::set_consent))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            handlers::require_token,
        ));

    Router::new()
        .route("/healthz", get(handlers::healthz))
        .route("/metrics", get(handlers::metrics))
        .route("/api/v1/consent/{distinct_id}", get(handlers::get_consent))
        .route("/api/v1/deliveries", get(handlers::deliveries))
        .merge(authenticated)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(ConcurrencyLimitLayer::new(concurrency))
        .with_state(state)
}
