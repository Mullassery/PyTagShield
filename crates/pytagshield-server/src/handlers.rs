use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use pytagshield_core::{
    EventContext, IngestOutcome, PipelineError, ValidationError,
};

use crate::app::AppState;

/// Handler-level errors mapped to explicit HTTP responses.
pub enum AppError {
    Validation(ValidationError),
    Unauthorized,
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Validation(err) => (StatusCode::BAD_REQUEST, Json(json!({
                "error": "validation_failed",
                "errors": err.errors,
            })))
                .into_response(),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized", "message": "missing or invalid bearer token"})),
            )
                .into_response(),
            AppError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "not_found", "message": msg})),
            )
                .into_response(),
            AppError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "bad_request", "message": msg})),
            )
                .into_response(),
            AppError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal", "message": msg})),
            )
                .into_response(),
        }
    }
}

fn internalize<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Internal(e.to_string())
}

/// Optional bearer-token authentication for ingest and consent writes.
pub async fn require_token(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let Some(expected) = state.config.ingest.token.as_deref() else {
        return Ok(next.run(request).await);
    };

    let headers = request.headers();
    let presented = bearer_token(headers).or_else(|| api_key_header(headers));

    match presented {
        Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => {
            Ok(next.run(request).await)
        }
        _ => {
            pytagshield_core::Metrics::inc(&state.metrics.requests_denied);
            Err(AppError::Unauthorized)
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    Some(token.trim().to_owned())
}

fn api_key_header(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("x-api-key")?.to_str().ok()?;
    Some(value.trim().to_owned())
}

/// Length-independent comparison to avoid leaking token prefixes via timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Still walk the shorter buffer to keep timing flat-ish.
        let mut diff = a.len() ^ b.len();
        for i in 0..a.len().min(b.len()) {
            diff |= (a[i] ^ b[i]) as usize;
        }
        return diff == 0;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Transport context derived from proxy headers. The IP is only ever kept
/// when minimization.forward_ip is enabled (cleared later in the pipeline).
fn context_from_headers(headers: &HeaderMap) -> EventContext {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        });
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    EventContext { ip, user_agent }
}

fn parse_json(
    result: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Value, AppError> {
    match result {
        Ok(Json(value)) => Ok(value),
        Err(rejection) => Err(AppError::BadRequest(format!(
            "request body is not valid JSON: {rejection}"
        ))),
    }
}

/// `POST /api/v1/event` — ingest a single event.
pub async fn ingest_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let raw = parse_json(body)?;
    let context = context_from_headers(&headers);

    match pytagshield_core::pipeline::process_and_enqueue(
        &state.pipeline,
        &state.queues,
        &raw,
        context,
    ) {
        Ok(outcome) => {
            let status = if outcome.status == "backpressure" {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::ACCEPTED
            };
            Ok((status, Json(outcome)).into_response())
        }
        Err(PipelineError::Validation(err)) => Err(AppError::Validation(err)),
        Err(e @ PipelineError::Config(_)) => Err(internalize(e)),
    }
}

#[derive(Deserialize)]
struct BatchEnvelope {
    events: Option<Vec<Value>>,
}

/// `POST /api/v1/batch` — ingest a batch of events.
///
/// The envelope always yields 200 with per-item results unless the envelope
/// itself is invalid (400) or authentication fails (401). Per-item failures
/// are explicit in `results[].error` and summarized in `summary`.
pub async fn ingest_batch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let raw = parse_json(body)?;
    let events: Vec<Value> = match &raw {
        Value::Array(items) => items.clone(),
        Value::Object(map) => {
            let envelope: BatchEnvelope =
                serde_json::from_value(raw.clone()).map_err(|e| {
                    AppError::BadRequest(format!(
                        "batch body must be an array or an object with an 'events' array: {e}"
                    ))
                })?;
            let Some(items) = envelope.events else {
                return Err(AppError::BadRequest(
                    "batch body must be an array or an object with an 'events' array".into(),
                ));
            };
            items
        }
        _ => {
            return Err(AppError::BadRequest(
                "batch body must be an array or an object with an 'events' array".into(),
            ))
        }
    };

    let max = state.config.ingest.max_batch_size;
    if events.len() > max {
        return Err(AppError::BadRequest(format!(
            "batch contains {} events; maximum is {max}",
            events.len()
        )));
    }

    let context = context_from_headers(&headers);
    let mut results = Vec::with_capacity(events.len());
    let mut summary = json!({
        "accepted": 0usize,
        "validation_failed": 0usize,
        "backpressure": 0usize,
        "partially_accepted": 0usize,
    });

    for (index, event) in events.into_iter().enumerate() {
        let item = match pytagshield_core::pipeline::process_and_enqueue(
            &state.pipeline,
            &state.queues,
            &event,
            context.clone(),
        ) {
            Ok(outcome) => match outcome.status {
                "accepted" => summary["accepted"] = json!(summary["accepted"].as_u64().unwrap_or(0) + 1),
                "backpressure" => {
                    summary["backpressure"] =
                        json!(summary["backpressure"].as_u64().unwrap_or(0) + 1)
                }
                _ => {
                    summary["partially_accepted"] =
                        json!(summary["partially_accepted"].as_u64().unwrap_or(0) + 1)
                }
            },
            Err(PipelineError::Validation(err)) => {
                summary["validation_failed"] =
                    json!(summary["validation_failed"].as_u64().unwrap_or(0) + 1);
                json!({"index": index, "error": "validation_failed", "errors": err.errors})
            }
            Err(e) => json!({"index": index, "error": "internal", "message": e.to_string()}),
        };
        results.push(item);
    }

    Ok(Json(json!({"summary": summary, "results": results})).into_response())
}

#[derive(Deserialize)]
struct SetConsentBody {
    distinct_id: String,
    purposes: std::collections::BTreeMap<String, bool>,
}

/// `POST /api/v1/consent` — store consent for a distinct id.
pub async fn set_consent(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let raw = parse_json(body)?;
    let body: SetConsentBody = serde_json::from_value(raw)
        .map_err(|e| AppError::BadRequest(format!("invalid consent payload: {e}")))?;

    let distinct_id = body.distinct_id.trim();
    if distinct_id.is_empty() || distinct_id.chars().count() > state.config.ingest.max_distinct_id_len
    {
        return Err(AppError::BadRequest(format!(
            "'distinct_id' must be 1..={} characters",
            state.config.ingest.max_distinct_id_len
        )));
    }

    let known = &state.config.consent.purposes;
    let mut ignored: Vec<String> = Vec::new();
    let mut purposes = std::collections::BTreeMap::new();
    for (key, value) in body.purposes {
        if known.contains(&key) {
            purposes.insert(key, value);
        } else {
            ignored.push(key);
        }
    }

    let now = chrono::Utc::now();
    let evicted = state.pipeline.consent_store().set(
        distinct_id,
        purposes.clone(),
        now,
        state.config.consent.store_ttl_secs,
        state.config.consent.store_max_entries,
    );

    Ok(Json(json!({
        "stored": true,
        "distinct_id": distinct_id,
        "purposes": purposes,
        "ignored_purposes": ignored,
        "ttl_secs": state.config.consent.store_ttl_secs,
        "evicted": evicted,
    }))
    .into_response())
}

/// `GET /api/v1/consent/{distinct_id}` — read stored consent.
pub async fn get_consent(
    State(state): State<Arc<AppState>>,
    Path(distinct_id): Path<String>,
) -> Result<Response, AppError> {
    let now = chrono::Utc::now();
    match state.pipeline.consent_store().get(
        &distinct_id,
        now,
        state.config.consent.store_ttl_secs,
    ) {
        Some(purposes) => Ok(Json(json!({
            "distinct_id": distinct_id,
            "purposes": purposes,
            "source": "stored",
        }))
        .into_response()),
        None => Err(AppError::NotFound(format!(
            "no stored consent for '{distinct_id}'"
        ))),
    }
}

#[derive(Deserialize)]
struct DeliveriesQuery {
    limit: Option<usize>,
}

/// `GET /api/v1/deliveries` — recent delivery outcomes (newest first).
pub async fn deliveries(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DeliveriesQuery>,
) -> Result<Response, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let items = state.delivery_log.recent(limit);
    Ok(Json(json!({
        "items": items,
        "retained": state.delivery_log.len(),
        "capacity": state.config.delivery.log_capacity,
        "limit": limit,
    }))
    .into_response())
}

/// `GET /healthz` — liveness and build information.
pub async fn healthz(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": state.uptime_secs(),
        "destinations_enabled": state.queues.len(),
        "consent_entries": state.pipeline.consent_store().len(),
    }))
}

/// `GET /metrics` — Prometheus text exposition.
pub async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        state.metrics.render_prometheus(),
    )
}
