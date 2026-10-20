# PyTagShield

Python bindings for the PyTagShield pipeline: a fast, consent-aware,
privacy-minimizing event pipeline written in Rust.

The core runs the latency-sensitive stages of a tagging pipeline — ingestion
validation, consent resolution, deterministic transforms, data minimization,
and routing to destinations — while staying free of network I/O until the
delivery stage. These bindings expose the synchronous pipeline so you can
validate, minimize, and route events from Python without running a server.

## Install

```bash
pip install pytagshield
```

Requires CPython 3.10+ (abi3 wheel).

## Quick start

```python
import pytagshield

CONFIG = """
minimization:
  pseudonymize_salt: "change-me"
destinations:
  - name: ph
    type: posthog
    purpose: analytics
    posthog:
      api_key: "phc_..."
routes:
  - name: all
    destinations: [ph]
"""

config = pytagshield.Config.from_yaml(CONFIG)
pipeline = pytagshield.Pipeline(config)

result = pipeline.process(
    {
        "event": "page_view",
        "distinct_id": "user-42",
        "properties": {"email": "a@b.c", "url": "/home"},
        "consent": {"analytics": True},
    }
)

print(result["routing"]["destinations"])        # ['ph']
print(result["event"]["distinct_id"])           # 'sha256:...' (pseudonymized)
print(result["minimization"]["properties_dropped"])  # 1 (email dropped)
```

## Design

- **Consent-first.** Nothing routes to a destination unless its purpose is
  granted. The default policy is `deny`; consent resolves with precedence
  event payload > stored consent > configured default.
- **Privacy by default.** Identifiers are pseudonymized (SHA-256 + salt) and
  PII such as IP addresses and email is cleared or dropped before egress.
- **Explicit backpressure.** `process()` is pure and side-effect free. Queueing
  to destinations is the transport layer's responsibility, so overload is
  never silent.

## API

| Object | Members |
|---|---|
| `Config` | `from_yaml`, `from_path`, `purposes`, `default_policy` |
| `Pipeline` | `process`, `set_consent`, `get_consent`, `consent_entries`, `metrics` |

`ValidationError` (with a `.errors` list) is raised for rejected payloads;
`ConfigError` is raised for invalid configuration.

## License

Apache-2.0. See [LICENSE](../../LICENSE).
