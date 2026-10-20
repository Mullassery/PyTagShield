//! Micro-benchmark for the synchronous pipeline stages.
//!
//! Run with `cargo bench -p pytagshield-core`. The harness is hand-rolled
//! (no external benchmark framework) to keep the dependency tree small and
//! builds reproducible.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use pytagshield_core::{Config, ConsentStore, EventContext, Metrics, Pipeline};
use serde_json::{json, Value};

fn test_config() -> Arc<Config> {
    let yaml = r#"
minimization:
  pseudonymize_salt: "bench-salt"
  drop_properties: [email, phone, ssn]
transforms:
  - when:
      events: ["page_*"]
    actions:
      - action: rename
        from: url
        to: page_url
      - action: map
        key: status
        from:
          "200": ok
          "404": missing
      - action: set
        key: source
        value: server
destinations:
  - name: posthog
    type: posthog
    purpose: analytics
    posthog:
      api_key: "phc_bench"
      host: "https://us.i.posthog.com"
  - name: webhook
    type: webhook
    purpose: advertising
    webhook:
      url: "https://example.com/hook"
routes:
  - name: pages
    destinations: [posthog]
    match:
      events: ["page_*"]
  - name: everything
    destinations: [posthog, webhook]
"#;
    Arc::new(Config::from_yaml(yaml).expect("bench config"))
}

fn sample_events() -> Vec<Value> {
    vec![
        json!({
            "event": "page_view",
            "distinct_id": "user-1",
            "timestamp": 1768488000000i64,
            "properties": {
                "url": "/products/42",
                "status": 200,
                "email": "person@example.com",
                "referrer": "https://news.example.com",
                "debug": {"verbose": true, "trace": "abc"}
            },
            "consent": {"analytics": true, "advertising": true}
        }),
        json!({
            "event": "purchase",
            "distinct_id": "user-2",
            "properties": {"revenue": 19.99, "currency": "USD", "phone": "555-0100"},
            "consent": {"analytics": true, "advertising": false}
        }),
        json!({
            "event": "signup_completed",
            "distinct_id": "user-3",
            "properties": {"plan": "pro", "email": "a@b.c"}
        }),
    ]
}

fn run(name: &str, iterations: usize, pipeline: &Pipeline, events: &[Value]) {
    let ctx = EventContext::default();
    let start = Instant::now();
    for i in 0..iterations {
        let raw = &events[i % events.len()];
        let processed = black_box(pipeline.process(raw, ctx.clone()).expect("valid event"));
        black_box(&processed.routing);
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed.as_nanos() as f64 / iterations as f64;
    let throughput = iterations as f64 / elapsed.as_secs_f64();
    println!(
        "{name:<28} {iterations:>8} iters  {per_iter:>10.1} ns/iter  {throughput:>12.0} events/s"
    );
}

fn main() {
    let metrics = Arc::new(Metrics::default());
    let pipeline = Pipeline::new(test_config(), Arc::new(ConsentStore::new()), metrics);
    let events = sample_events();

    // Warmup: page caches, lazy allocations, counter cells.
    run("warmup", 1_000, &pipeline, &events);
    run("pipeline (100k)", 100_000, &pipeline, &events);

    // Metrics snapshot proves the bench actually executed the pipeline.
    let m = pipeline.metrics();
    assert!(m.events_accepted.load(std::sync::atomic::Ordering::Relaxed) >= 101_000);
    let _ = BTreeMap::<u8, u8>::new();
}
