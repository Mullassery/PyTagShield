# Audit findings — 2026-10-26

This file records the current portfolio-audit findings for PyTagShield. It is
documentation only; no code or release artifacts were changed. The fuller
evidence report is kept in the portfolio workspace at
`.audit/reports/PyTagShield/audit.md`.

## What it is

A Rust consent-aware analytics data plane with a Python API and Axum HTTP
server: validate -> resolve consent -> transform -> minimize -> route -> bounded
delivery to PostHog, GA4, Meta CAPI, sGTM, webhooks, or a local log.

## Verified strengths

- Deny-by-default consent with per-destination purpose gates.
- Salted SHA-256 pseudonymization before egress; PII-like keys dropped by
  default; property/depth/string caps; IP cleared unless explicitly forwarded.
- Synchronous, side-effect-free `process()` separated from async delivery.
- Bounded per-destination queues, explicit 429 backpressure, retries with
  exponential backoff, and a bounded delivery log.
- Strict YAML config (`deny_unknown_fields`); required pseudonymization salt.
- Bearer/`X-API-Key` auth for ingest and consent writes with constant-time
  comparison.
- Prometheus `/metrics`, `/healthz`, delivery inspection, graceful shutdown.
- 113 static Rust tests, matching the README; CI runs fmt, clippy, tests,
  Python binding smoke test, and dependency audit.

## Verified gaps and user-facing risks

- **Unauthenticated read endpoints:** `GET /api/v1/consent/{distinct_id}` and
  `GET /api/v1/deliveries` sit outside the token middleware. When the server is
  reachable, consent decisions and recent event/delivery metadata are exposed
  even with an ingest token configured.
- **Unconditional proxy-header trust:** `X-Forwarded-For`/`X-Real-IP` are read
  from any peer. With `forward_ip: true`, a direct client can spoof the
  forwarded IP.
- **Optional ingest auth:** `ingest.token` defaults to `None`; safe only on
  loopback.
- **In-memory delivery queue:** accepted events in the queue are lost on
  crash/restart. There is no durable queue or dead-letter store.
- **No destination allowlist or built-in TLS listener:** requires a trusted
  gateway and egress policy.
- **Plaintext config secrets:** destination API keys and the pseudonymization
  salt are config values with no secret-manager indirection.
- **Single worker per destination:** throughput ceiling per sink.
- **Pseudonymized IDs are still personal data;** no salt rotation.
- **Python surface is lightly tested:** one binding smoke test.
- Metadata says Beta while README says alpha.

## Direct positioning

A small, auditable data plane rather than a consent-management platform,
warehouse, or analytics backend. Its differentiator is fail-closed consent plus
deterministic minimization before egress.

## Ranked next actions

1. P0: put consent/delivery GET routes behind auth, or remove them from the
   public surface.
2. P0: trust proxy headers only from configured CIDRs.
3. P1: require an ingest token for non-loopback binds, or refuse startup.
4. P1: add a durable queue or explicitly document at-most-once-on-crash and a
   caller replay path.
5. P1: add a dead-letter store and end-to-end mock-destination tests.
6. P2: add TLS or document a mandatory trusted gateway with TLS termination.
7. P2: add secret-manager/env indirection and salt rotation guidance.
8. P2: add fuzz tests for event parsing/config and expand Python tests.

## Do not build

- A consent-collection UI, a full CDP, or a durable multi-tenant event bus.
  Stay a compact privacy-preserving delivery data plane.

## Evidence gaps / validation needed

- No live destination delivery/retry integration test.
- No load or saturation test.
- No fuzz/property testing of event parsing.
- No independent verification of PyPI wheel availability.

## Recommended disposition

Maintain as a promising alpha component. Fix the read-endpoint exposure and
proxy-header trust before recommending any network-facing deployment.
