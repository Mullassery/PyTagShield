# Cross-Repository Blockers — PyTagShield ↔ Squid-CRM

Detailed specification of the PyTagShield → Squid-CRM integration blocker,
from the PyTagShield perspective. The shared context is documented in
`Mullassery/Squid-CRM/docs/cross-repository-blockers.md`; this file records the
PyTagShield side of the contract and what must change here to unblock it.

Reference pin: `twentyhq/twenty@08a0e5f1ee2e7db7c1e6ef3aeec9bcef731f4e98`
(Twenty server `2.50.0`).

## 1. The blocker

**Status:** RESOLVED IN WORKING TREE (server + contract verified locally) —
Consent decisions attach to **events**, not Squid records. The server binary is
now runnable, and outbound events carry a versioned `pytagshield.consent.v1`
block. Live Squid verification remains outstanding.

## 2. The two sides

| Aspect | Squid-CRM (CRM destination) | PyTagShield (consent authority) |
|--------|----------------------------|---------------------------------|
| Role | System of record for CRM records; stores PII by design | Consent-aware event validation, pseudonymization, routing; fail-closed by default |
| Transport | Record APIs (REST/GraphQL), custom objects; webhook outbound with envelope v1 | `POST /api/v1/event` with `event`, `distinct_id`, `timestamp`, `properties`, optional `consent` |
| Consent metadata | Not emitted on webhook envelopes; no field-level PII minimization | Consent is a gate: destination receives event only when required purpose is granted |
| PII handling | No field-level PII masking in webhook payloads | Drop/allowlist properties, truncate strings, clear IPs, pseudonymize identifiers with salted hash |
| Audit trail | `core.webhook_deliveries` log with retry/terminal state | Bounded per-destination queues, delivery log |
| Runtime contract | Axum + Tokio, PostgreSQL, schema-per-workspace | Rust core + Python bindings; runnable Axum HTTP server |

## 3. Agreed contract (PyTagShield side)

1. **Consent attachment point.** Decided: **events only**. Squid records do not
   carry consent columns. A deployment may store an opaque decision/event
   reference in a custom object, but never a boolean grant.
2. **Consent schema.** Implemented as `pytagshield.consent.v1` on outbound
   events: `consent` (granted), `consent_decisions` (full boolean map), and
   `consent_source` (`event|stored|default`).
3. **PII minimization.** PyTagShield policy decides what survives before
   egress. Squid must not widen that allowlist or re-introduce dropped fields.
4. **Consent authority boundary.** PyTagShield is the decision point. Squid
   only receives data after that gate passes and must not infer consent from a
   missing field.
5. **Event routing.** A Squid webhook entering PyTagShield is an input, not a
   consent decision. PyTagShield re-resolves consent from event/stored/default
   before forwarding to any destination.

## 4. Evidence

**PyTagShield:**
- Rust core + Python bindings + runnable server (113 tests in the workspace).
- PyPI wheels currently target CPython 3.10+ on macOS arm64; other platforms
  build from source.
- The HTTP server binary is now implemented and covered by router tests; it
  loads and validates YAML config and runs until SIGINT/SIGTERM.
- Consent is a gate, not a report: default policy is `deny`, precedence is
  explicit (event payload → stored consent → configured default).
- Minimize before sending: drop or allowlist properties, cap count and depth,
  truncate strings, clear IPs, pseudonymize identifiers with salted hash.
- Bounded delivery: per-destination queues, explicit backpressure, timeouts,
  exponential backoff, bounded delivery log.

**Squid-CRM:**
- Webhook envelope v1 has no consent decisions and no field-level PII
  minimization metadata. Documented as a known gap.
- Squid does not decide consent; PyTagShield or another consent authority must
  gate what is sent to Squid or emitted onward.

## 5. Owner of next move

**PyTagShield:** implemented the event-only consent contract and the runnable
server. See `docs/squid-consent-contract.md`. **Squid-CRM:** no consent columns
or consent authority are required; live ingestion verification remains.

## 6. Related blockers

- **PyReverseETL + Squid:** activation payload + idempotency header — gates
  warehouse→CRM activation (independent of consent).
- **PyCustomerHub + Squid:** shared event-schema contract — gates Squid webhook
  delivery to the CDP.
- **ClusterAudienceKit / PyTagManager + Squid:** custom-object schemas — gates
  audience/finding writes to Squid custom objects.
