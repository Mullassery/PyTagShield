# Cross-Repository Blockers — PyTagShield ↔ Squid-CRM

Detailed specification of the PyTagShield → Squid-CRM integration blocker,
from the PyTagShield perspective. The shared context is documented in
`Mullassery/Squid-CRM/docs/cross-repository-blockers.md`; this file records the
PyTagShield side of the contract and what must change here to unblock it.

Reference pin: `twentyhq/twenty@08a0e5f1ee2e7db7c1e6ef3aeec9bcef731f4e98`
(Twenty server `2.50.0`).

## 1. The blocker

**Status:** UNVERIFIED / OPEN — No consent decision contract between PyTagShield
and Squid-CRM. At audit time the PyTagShield GitHub repository had no README and
no workflows; the server binary was previously found to be a no-op
(`fn main() {}`), so a runnable HTTP service contract is not evidenced. There is
no Squid-CRM destination adapter, and no executable evidence that consent
decisions are attached to outbound CRM records.

## 2. The two sides

| Aspect | Squid-CRM (CRM destination) | PyTagShield (consent authority) |
|--------|----------------------------|---------------------------------|
| Role | System of record for CRM records; stores PII by design | Consent-aware event validation, pseudonymization, routing; fail-closed by default |
| Transport | Record APIs (REST/GraphQL), custom objects; webhook outbound with envelope v1 | `POST /api/v1/event` with `event`, `distinct_id`, `timestamp`, `properties`, optional `consent` |
| Consent metadata | Not emitted on webhook envelopes; no field-level PII minimization | Consent is a gate: destination receives event only when required purpose is granted |
| PII handling | No field-level PII masking in webhook payloads | Drop/allowlist properties, truncate strings, clear IPs, pseudonymize identifiers with salted hash |
| Audit trail | `core.webhook_deliveries` log with retry/terminal state | Bounded per-destination queues, delivery log |
| Runtime contract | Axum + Tokio, PostgreSQL, schema-per-workspace | Rust core + Python bindings; HTTP server bootstrap still being wired |

## 3. What must be agreed (PyTagShield side)

1. **Consent attachment point.** Do consent decisions travel on CRM records (as
   fields), on webhook events (as envelope metadata), or both? This determines
   whether Squid stores consent metadata on records or emits it on outbound
   webhooks.
2. **Consent schema.** If consent metadata is stored or emitted, what is its
   shape? (purpose grants, timestamps, provenance, source, version.)
3. **PII minimization.** Which CRM fields are minimized/pseudonymized before
   webhook emission? Who decides — PyTagShield policy or Squid configuration?
4. **Consent authority boundary.** Squid must not be treated as a consent
   authority. PyTagShield must be the decision point; Squid only stores/forwards
   what PyTagShield approves.
5. **Event routing.** Does Squid act as a destination for PyTagShield-processed
   events, or does Squid emit events that PyTagShield then filters?

## 4. Evidence

**PyTagShield:**
- Rust core + Python bindings (110 tests in the workspace).
- PyPI wheels currently target CPython 3.10+ on macOS arm64; other platforms
  build from source.
- The HTTP server has a tested router and handlers, but its binary bootstrap is
  still being wired.
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

**PyTagShield** must define whether consent decisions attach to records, events,
or both, and provide a runnable HTTP service contract (the server binary was a
no-op at audit time). Squid-CRM cannot add consent metadata until this is
defined.

## 6. Related blockers

- **PyReverseETL + Squid:** activation payload + idempotency header — gates
  warehouse→CRM activation (independent of consent).
- **PyCustomerHub + Squid:** shared event-schema contract — gates Squid webhook
  delivery to the CDP.
- **ClusterAudienceKit / PyTagManager + Squid:** custom-object schemas — gates
  audience/finding writes to Squid custom objects.
