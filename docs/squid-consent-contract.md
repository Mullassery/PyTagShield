# PyTagShield ↔ Squid-CRM consent contract

**Status:** resolved in the PyTagShield working tree. Live cross-service
verification remains outstanding.

## Decision

Consent decisions attach to **events**, not to Squid CRM records.

PyTagShield is the consent authority. Squid is a system of record for CRM
data; it must not adjudicate consent or emit a boolean "consented" field as if
it were the authority.

This means:

- Every PyTagShield-approved outbound event carries a versioned consent block.
- Squid record payloads do not need consent fields, and PyTagShield must never
  invent one.
- If a deployment needs to associate a record with a decision, it may store an
  opaque decision/event reference in a custom object. It must not promote that
  reference into a consent grant.
- If Squid emits a webhook into PyTagShield, that envelope is an input, not a
  consent decision. PyTagShield resolves consent from the event payload, its
  stored consent, or the configured default, in that order.

## Outbound event shape

The webhook destination emits the existing event envelope plus
`pytagshield.consent.v1` metadata:

```json
{
  "schema": "pytagshield.event.v1",
  "consent_schema": "pytagshield.consent.v1",
  "event_id": "<uuid>",
  "event": "page_view",
  "distinct_id": "<pseudonymized-id>",
  "timestamp_millis": 1790000000000,
  "timestamp": "2026-10-23T12:00:00Z",
  "properties": {},
  "consent": {
    "analytics": true
  },
  "consent_decisions": {
    "analytics": true,
    "advertising": false,
    "functional": true,
    "personalization": false
  },
  "consent_source": "stored",
  "context": {}
}
```

Field semantics:

| Field | Meaning |
|---|---|
| `consent_schema` | Version of the consent block. |
| `consent` | Granted purposes only. Kept for existing consumers. |
| `consent_decisions` | Full decision for every configured purpose, including denials. |
| `consent_source` | `event`, `stored`, or `default` — where the effective value came from. |

Denied purposes are never inferred from omission in `consent_decisions`; the
boolean is explicit.

## Authority boundary

1. PyTagShield resolves consent before any destination dispatch.
2. A destination is skipped when its required purpose is denied.
3. Squid only receives data after that gate has passed.
4. Squid must not treat a missing consent field as consent, and must not
   re-grant consent based on its own configuration.
5. Downstream systems should consume the event consent block directly rather
   than asking Squid to reconstruct it.

## Server contract

The HTTP server is now runnable:

```bash
cargo run -p pytagshield-server -- /path/to/config.yaml
# or
PYTAGSHIELD_CONFIG=/path/to/config.yaml cargo run -p pytagshield-server
```

It loads and validates the strict YAML config, builds the bounded delivery
workers, serves `POST /api/v1/event`, `POST /api/v1/batch`,
`POST /api/v1/consent`, the read endpoints, `/healthz`, and `/metrics`, and
shuts down gracefully on SIGINT/SIGTERM.

## Remaining work

- Live end-to-end verification with a real Squid workspace.
- If PyTagShield later writes records directly to Squid, that adapter must
  forward only approved fields and must not add consent columns.
- Persistent consent storage (Redis/Postgres) remains a PyTagShield roadmap
  item; the current store is bounded in-memory.