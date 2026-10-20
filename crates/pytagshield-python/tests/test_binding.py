import json
import pytagshield

CONFIG = """
minimization:
  pseudonymize_salt: "test-salt"
destinations:
  - name: ph
    type: posthog
    purpose: analytics
    posthog:
      api_key: "phc_test"
  - name: ads
    type: log
    purpose: advertising
routes:
  - name: all
    destinations: [ph, ads]
"""

cfg = pytagshield.Config.from_yaml(CONFIG)
print("purposes:", cfg.purposes)
print("default_policy:", cfg.default_policy)
assert cfg.default_policy == "deny"
assert "analytics" in cfg.purposes

pipe = pytagshield.Pipeline(cfg)

# 1. No consent -> nothing routes, but the event still completes the pipeline.
out = pipe.process({"event": "page_view", "distinct_id": "user-42",
                    "properties": {"email": "a@b.c", "url": "/home", "debug": True}})
print("no-consent dest:", out["routing"]["destinations"],
      "skipped:", len(out["routing"]["skipped"]))
assert out["routing"]["destinations"] == []
assert len(out["routing"]["skipped"]) == 2
assert all(s["reason"].startswith("consent_denied") for s in out["routing"]["skipped"])
assert out["consent"]["source"] == "default"

# 2. Partial consent -> only analytics destination.
out = pipe.process({"event": "page_view", "distinct_id": "user-42",
                    "properties": {"email": "a@b.c", "url": "/home"},
                    "consent": {"analytics": True, "advertising": False}})
print("partial dest:", out["routing"]["destinations"],
      "pseudonymized:", out["minimization"]["id_pseudonymized"])
assert out["routing"]["destinations"] == ["ph"]
assert out["minimization"]["id_pseudonymized"] is True
assert out["consent"]["source"] == "event"
assert out["consent"]["purposes"]["advertising"] is False

# 3. Full consent -> both, email dropped, id pseudonymized.
out = pipe.process({"event": "page_view", "distinct_id": "user-42",
                    "properties": {"email": "a@b.c", "url": "/home"},
                    "consent": {"analytics": True, "advertising": True}})
print("full dest:", out["routing"]["destinations"],
      "dropped:", out["minimization"]["properties_dropped"])
assert out["routing"]["destinations"] == ["ph", "ads"]
assert out["minimization"]["properties_dropped"] == 1
assert out["event"]["distinct_id"].startswith("sha256:")
assert "email" not in out["event"]["properties"]

# 4. Validation error carries .errors
try:
    pipe.process({"properties": {}})
    raise AssertionError("expected ValidationError")
except pytagshield.ValidationError as e:
    print("validation errors:", e.errors)
    assert any("'event'" in x for x in e.errors)
    assert any("'distinct_id'" in x for x in e.errors)

# 5. Unsupported Python types are rejected.
try:
    pipe.process({"event": "e", "distinct_id": "u", "properties": {"s": {1, 2, 3}}})
    raise AssertionError("expected TypeError for set")
except TypeError as e:
    print("non-JSON type rejected OK")

# 6. Consent store roundtrip.
res = pipe.set_consent("user-99", {"analytics": True, "advertising": False, "nonsense": True})
print("set_consent:", res)
assert res["purposes"] == {"analytics": True, "advertising": False}
assert res["ignored_purposes"] == ["nonsense"]
got = pipe.get_consent("user-99")
assert got["purposes"] == {"analytics": True, "advertising": False}
assert pipe.get_consent("nobody") is None
assert pipe.consent_entries == 1
print("consent_entries:", pipe.consent_entries)

# 7. Non-bool purpose value is a TypeError.
try:
    pipe.set_consent("user-99", {"analytics": "yes"})
    raise AssertionError("expected TypeError")
except TypeError:
    print("non-bool purpose rejected OK")

# 8. Stored consent is used when the event is silent.
out = pipe.process({"event": "click", "distinct_id": "user-99", "properties": {}})
print("stored-consent source:", out["consent"]["source"], "dest:", out["routing"]["destinations"])
assert out["consent"]["source"] == "stored"
assert out["routing"]["destinations"] == ["ph"]

# 9. Transport context is attached, but the IP is cleared by minimization
# (PII must not reach destinations); the user agent survives by default.
out = pipe.process({"event": "click", "distinct_id": "user-99"},
                   ip="1.2.3.4", user_agent="pytest")
print("context:", out["event"]["context"])
assert out["event"]["context"].get("ip") is None
assert out["event"]["context"]["user_agent"] == "pytest"
assert out["minimization"]["ip_cleared"] is True

# 10. Config errors surface as ConfigError.
try:
    pytagshield.Config.from_yaml("destinatins: []")
    raise AssertionError("expected ConfigError")
except pytagshield.ConfigError:
    print("config error raised OK")

# 11. Metrics render in Prometheus format.
m = pipe.metrics()
print("metrics sample:", m.splitlines()[0])
assert "pytagshield" in m

# 12. Round-trip through JSON is stable (result is plain Python data).
json.dumps(out)
print("\nALL PYTHON BINDING CHECKS PASSED")
