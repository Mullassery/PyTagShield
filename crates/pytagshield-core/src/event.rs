use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::IngestConfig;
use crate::error::ValidationError;

/// Transport-level context extracted by the server and attached before the
/// pipeline runs. It is never forwarded to a destination unless explicitly
/// allowed by configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

/// The normalized internal representation of an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Server-generated identifier (UUIDv4).
    pub id: String,
    /// Event name as supplied by the client.
    pub name: String,
    /// Stable identifier of the subject. Pseudonymized before egress when
    /// minimization.pseudonymize_ids is enabled.
    pub distinct_id: String,
    /// Event time in Unix milliseconds (UTC).
    pub timestamp_millis: i64,
    /// Event properties after validation (minimization and transforms run
    /// later in the pipeline).
    pub properties: BTreeMap<String, Value>,
    /// Per-event consent overrides, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<BTreeMap<String, bool>>,
    #[serde(default)]
    pub context: EventContext,
}

impl Event {
    /// Validate and normalize a raw ingest payload.
    ///
    /// Accepted input shape (PostHog-inspired):
    /// `{ "event": "...", "distinct_id": "...", "timestamp": ..., "properties": {...}, "consent": {...} }`
    pub fn parse(
        raw: &Value,
        cfg: &IngestConfig,
        now: DateTime<Utc>,
    ) -> Result<Self, ValidationError> {
        let mut errors: Vec<String> = Vec::new();

        let obj = match raw.as_object() {
            Some(o) => o,
            None => {
                return Err(ValidationError::single(
                    "event payload must be a JSON object",
                ))
            }
        };

        // `event` is canonical; `name` is accepted as an alias.
        let name_raw = obj.get("event").or_else(|| obj.get("name"));
        match name_raw {
            None => errors.push("missing required field 'event'".into()),
            Some(Value::String(s)) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    errors.push("'event' must not be empty".into());
                } else if trimmed.chars().count() > cfg.max_event_name_len {
                    errors.push(format!(
                        "'event' exceeds {} characters",
                        cfg.max_event_name_len
                    ));
                } else if trimmed.chars().any(char::is_control) {
                    errors.push("'event' must not contain control characters".into());
                }
            }
            Some(_) => errors.push("'event' must be a string".into()),
        }

        let distinct_id = match obj.get("distinct_id") {
            None => {
                if cfg.require_distinct_id {
                    errors.push("missing required field 'distinct_id'".into());
                }
                None
            }
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() {
                    errors.push("'distinct_id' must not be empty".into());
                    None
                } else if t.chars().count() > cfg.max_distinct_id_len {
                    errors.push(format!(
                        "'distinct_id' exceeds {} characters",
                        cfg.max_distinct_id_len
                    ));
                    None
                } else if t.chars().any(char::is_control) {
                    errors.push("'distinct_id' must not contain control characters".into());
                    None
                } else {
                    Some(t.to_owned())
                }
            }
            Some(_) => {
                errors.push("'distinct_id' must be a string".into());
                None
            }
        };

        let timestamp_millis = match parse_timestamp(obj.get("timestamp"), now) {
            Ok(ts) => {
                let skew_ms = cfg.max_timestamp_skew_secs.saturating_mul(1000);
                let now_ms = now.timestamp_millis();
                if ts > now_ms.saturating_add(skew_ms) {
                    errors.push(format!(
                        "'timestamp' is more than {} seconds in the future",
                        cfg.max_timestamp_skew_secs
                    ));
                    None
                } else if ts < now_ms.saturating_sub(skew_ms) {
                    errors.push(format!(
                        "'timestamp' is more than {} seconds in the past",
                        cfg.max_timestamp_skew_secs
                    ));
                    None
                } else {
                    Some(ts)
                }
            }
            Err(msg) => {
                errors.push(msg);
                None
            }
        };

        let mut properties: BTreeMap<String, Value> = BTreeMap::new();
        match obj.get("properties") {
            None | Some(Value::Null) => {}
            Some(Value::Object(map)) => {
                if map.len() > cfg.max_properties_per_event {
                    errors.push(format!(
                        "'properties' has {} keys; maximum is {}",
                        map.len(),
                        cfg.max_properties_per_event
                    ));
                }
                for (k, v) in map {
                    if k.is_empty() {
                        errors.push("property keys must not be empty".into());
                        continue;
                    }
                    if k.chars().count() > cfg.max_property_key_len {
                        errors.push(format!(
                            "property key '{k}' exceeds {} characters",
                            cfg.max_property_key_len
                        ));
                        continue;
                    }
                    if k.chars().any(char::is_control) {
                        errors.push(format!("property key '{k}' contains control characters"));
                        continue;
                    }
                    properties.insert(k.clone(), v.clone());
                }
            }
            Some(_) => errors.push("'properties' must be a JSON object".into()),
        }

        let consent = match obj.get("consent") {
            None | Some(Value::Null) => None,
            Some(Value::Object(map)) => {
                let mut consent = BTreeMap::new();
                let mut bad = false;
                for (k, v) in map {
                    match v.as_bool() {
                        Some(b) => {
                            consent.insert(k.clone(), b);
                        }
                        None => {
                            errors.push(format!(
                                "consent '{k}' must be true or false (got {v})"
                            ));
                            bad = true;
                        }
                    }
                }
                if bad {
                    None
                } else {
                    Some(consent)
                }
            }
            Some(_) => {
                errors.push("'consent' must be a JSON object of string -> boolean".into());
                None
            }
        };

        if !errors.is_empty() {
            return Err(ValidationError::new(errors));
        }

        let name = name_raw
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_owned();
        let distinct_id = distinct_id.unwrap_or_else(|| "_anonymous".to_owned());
        let timestamp_millis = timestamp_millis.unwrap_or_else(|| now.timestamp_millis());

        Ok(Event {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            distinct_id,
            timestamp_millis,
            properties,
            consent,
            context: EventContext::default(),
        })
    }

    /// RFC3339 rendering of the event timestamp.
    pub fn timestamp_rfc3339(&self) -> String {
        Utc.timestamp_millis_opt(self.timestamp_millis)
            .single()
            .map(|t| t.to_rfc3339())
            .unwrap_or_default()
    }
}

fn parse_timestamp(value: Option<&Value>, now: DateTime<Utc>) -> Result<i64, String> {
    match value {
        None | Some(Value::Null) => Ok(now.timestamp_millis()),
        Some(Value::Number(n)) => {
            if let Some(v) = n.as_i64() {
                Ok(normalize_epoch(v))
            } else if let Some(v) = n.as_f64() {
                if v.is_finite() {
                    Ok(normalize_epoch(v as i64))
                } else {
                    Err("'timestamp' must be a finite number".into())
                }
            } else {
                Err("'timestamp' must be a number or an RFC3339 string".into())
            }
        }
        Some(Value::String(s)) => {
            let t = s.trim();
            if let Ok(v) = t.parse::<i64>() {
                return Ok(normalize_epoch(v));
            }
            match DateTime::parse_from_rfc3339(t) {
                Ok(dt) => Ok(dt.timestamp_millis()),
                Err(_) => Err(format!(
                    "'timestamp' must be RFC3339 or Unix seconds/millis, got '{t}'"
                )),
            }
        }
        Some(_) => Err("'timestamp' must be a number or an RFC3339 string".into()),
    }
}

/// Heuristic: values >= 1e12 are treated as milliseconds, smaller as seconds.
fn normalize_epoch(v: i64) -> i64 {
    if v.abs() >= 1_000_000_000_000 {
        v
    } else {
        v.saturating_mul(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> IngestConfig {
        IngestConfig::default()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
    }

    fn ok(raw: &str) -> Event {
        let v: Value = serde_json::from_str(raw).unwrap();
        Event::parse(&v, &cfg(), now()).expect("valid event")
    }

    fn err(raw: &str) -> ValidationError {
        let v: Value = serde_json::from_str(raw).unwrap();
        Event::parse(&v, &cfg(), now()).expect_err("must be rejected")
    }

    #[test]
    fn accepts_well_formed_event() {
        let e = ok(
            r#"{"event":"page_view","distinct_id":"u1","properties":{"url":"/a"}}"#,
        );
        assert_eq!(e.name, "page_view");
        assert_eq!(e.distinct_id, "u1");
        assert_eq!(e.timestamp_millis, now().timestamp_millis());
        assert!(!e.id.is_empty());
    }

    #[test]
    fn accepts_name_alias() {
        let e = ok(r#"{"name":"signup","distinct_id":"u1"}"#);
        assert_eq!(e.name, "signup");
    }

    #[test]
    fn rejects_missing_event_and_distinct_id_together() {
        let e = err(r#"{"properties":{}}"#);
        assert!(e.errors.iter().any(|x| x.contains("'event'")));
        assert!(e.errors.iter().any(|x| x.contains("'distinct_id'")));
    }

    #[test]
    fn rejects_non_object_payload() {
        let v = Value::Array(vec![]);
        let e = Event::parse(&v, &cfg(), now()).expect_err("must fail");
        assert!(e.errors[0].contains("JSON object"));
    }

    #[test]
    fn rejects_empty_and_oversized_names() {
        assert!(err(r#"{"event":"  ","distinct_id":"u"}"#).errors[0].contains("empty"));
        let long = "x".repeat(200);
        let payload = format!(r#"{{"event":"{long}","distinct_id":"u"}}"#);
        assert!(err(&payload).errors[0].contains("exceeds"));
    }

    #[test]
    fn rejects_control_characters_in_event_name() {
        let payload = "{\"event\":\"a\\u0000b\",\"distinct_id\":\"u\"}";
        assert!(err(payload).errors[0].contains("control characters"));
    }

    #[test]
    fn rejects_bad_timestamps() {
        let e = err(r#"{"event":"e","distinct_id":"u","timestamp":"not-a-time"}"#);
        assert!(e.errors[0].contains("timestamp"));
        let e = err(r#"{"event":"e","distinct_id":"u","timestamp":99999999999}"#);
        assert!(e.errors[0].contains("future"));
        let e = err(r#"{"event":"e","distinct_id":"u","timestamp":1000000000}"#);
        assert!(e.errors[0].contains("past"));
    }

    #[test]
    fn accepts_epoch_millis_and_rfc3339() {
        let e = ok(r#"{"event":"e","distinct_id":"u","timestamp":1768488000000}"#);
        assert_eq!(e.timestamp_millis, 1768488000000);
        let e = ok(r#"{"event":"e","distinct_id":"u","timestamp":"2026-01-15T11:59:00Z"}"#);
        assert_eq!(e.timestamp_millis, 1768478340000); // 2026-01-15T11:59:00Z
    }

    #[test]
    fn rejects_bad_consent_values() {
        let e = err(r#"{"event":"e","distinct_id":"u","consent":{"analytics":"yes"}}"#);
        assert!(e.errors[0].contains("consent 'analytics'"));
    }

    #[test]
    fn rejects_bad_properties() {
        let e = err(r#"{"event":"e","distinct_id":"u","properties":[]}"#);
        assert!(e.errors[0].contains("must be a JSON object"));
        let payload =
            format!(r#"{{"event":"e","distinct_id":"u","properties":{{"{}":"v"}}}}"#, "k".repeat(300));
        let e = err(&payload);
        assert!(e.errors[0].contains("exceeds"));
    }

    #[test]
    fn rejects_property_count_over_limit() {
        let mut props = serde_json::Map::new();
        for i in 0..501 {
            props.insert(format!("p{i}"), Value::from(i));
        }
        let mut obj = serde_json::Map::new();
        obj.insert("event".into(), Value::from("e"));
        obj.insert("distinct_id".into(), Value::from("u"));
        obj.insert("properties".into(), Value::Object(props));
        let e = Event::parse(&Value::Object(obj), &cfg(), now()).expect_err("must fail");
        assert!(e.errors[0].contains("maximum is 500"));
    }

    #[test]
    fn timestamp_normalizes_seconds_to_millis() {
        assert_eq!(normalize_epoch(1_768_488_000), 1_768_488_000_000);
        assert_eq!(normalize_epoch(1_768_488_000_000), 1_768_488_000_000);
    }
}
