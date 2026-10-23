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

use pytagshield_core::{Config, ConsentStore, DeliveryLog, DeliveryTask, Metrics, Pipeline};

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
        Utc::now()
            .signed_duration_since(self.started_at)
            .num_seconds()
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn config(extra: &str) -> Config {
        Config::from_yaml(&format!(
            "minimization:\n  pseudonymize_salt: \"test-salt\"\n{extra}"
        ))
        .expect("valid test config")
    }

    #[tokio::test]
    async fn healthz_is_public_and_reports_server_state() {
        let app = build_router(build_state(config("")));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["destinations_enabled"], 0);
    }

    #[tokio::test]
    async fn ingest_requires_the_configured_bearer_token() {
        let app = build_router(build_state(config("ingest:\n  token: \"secret\"\n")));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/event")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"event":"page_view","distinct_id":"u1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_ingest_is_accepted() {
        let app = build_router(build_state(config("ingest:\n  token: \"secret\"\n")));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/event")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer secret")
                    .body(Body::from(r#"{"event":"page_view","distinct_id":"u1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
}
