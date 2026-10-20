//! Click-ID / attribution preservation.
//!
//! Marketing platforms attach click identifiers to landing-page URLs (and,
//! on the server side, expect to receive them back on conversion events).
//! Browsers and privacy features routinely strip these from the URL, so once
//! a client has captured one it is valuable to carry it forward explicitly.
//!
//! This stage is *reductive*: it copies click IDs the client already sent
//! from their original property keys into a canonical lowercase key, and
//! never fabricates values. That keeps it deterministic and safe to run
//! before routing, alongside `rename`/`drop`.

use serde::Serialize;
use serde_json::Value;

use crate::config::{AttributionConfig, DEFAULT_CLICK_ID_KEYS};
use crate::consent::ConsentSnapshot;
use crate::event::Event;

/// The canonical property key under which preserved click IDs are grouped.
pub const ATTRIBUTION_PROPERTY: &str = "$attribution";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AttributionReport {
    /// Number of click IDs newly preserved on this event.
    pub click_ids_preserved: usize,
    /// Names of the click IDs preserved (canonical, lowercase).
    pub preserved: Vec<String>,
    /// Whether consent gating skipped preservation entirely.
    pub skipped_for_consent: bool,
}

/// Preserve click identifiers found on the event into a canonical shape.
///
/// The canonical shape is a `$attribution` object mapping each recognised
/// click-ID name to its string value:
///
/// ```json
/// { "$attribution": { "gclid": "abc", "fbclid": "xyz" } }
/// ```
///
/// Recognition is case-insensitive: a client sending `GCLID` or `gClid` is
/// normalised to `gclid`. Values that are not scalars (objects/arrays) are
/// ignored, as are empty strings.
pub fn preserve(
    event: &mut Event,
    cfg: &AttributionConfig,
    consent: &ConsentSnapshot,
    report: &mut AttributionReport,
) {
    if !cfg.preserve_click_ids {
        return;
    }

    // Click IDs are attribution signals; when the operator demands consent,
    // honor it using the advertising purpose (falling back to any granted
    // purpose for instances that do not declare `advertising`).
    if cfg.require_consent && !click_id_consent_ok(consent) {
        report.skipped_for_consent = true;
        return;
    }

    let mut keys: Vec<String> = DEFAULT_CLICK_ID_KEYS
        .iter()
        .map(|k| k.to_ascii_lowercase())
        .collect();
    for extra in &cfg.click_id_keys {
        let lower = extra.trim().to_ascii_lowercase();
        if !lower.is_empty() && !keys.contains(&lower) {
            keys.push(lower);
        }
    }

    // Collect recognised click IDs from the event's top-level properties.
    // Matching is case-insensitive, so build a lowercase -> canonical map.
    let mut found: Vec<(String, String)> = Vec::new();
    for key in &keys {
        if let Some(value) = event.properties.get(key) {
            if let Some(s) = scalar_string(value) {
                if !s.is_empty() {
                    found.push((key.clone(), s));
                    continue;
                }
            }
        }
        // Fall back to a case-insensitive search for a differently-cased key.
        let mismatched = event
            .properties
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == *key)
            .map(|(k, v)| (k.clone(), v.clone()));
        if let Some((actual_key, value)) = mismatched {
            if let Some(s) = scalar_string(&value) {
                if !s.is_empty() {
                    // Normalise the original key to its canonical lowercase
                    // form so downstream consumers see one spelling.
                    if actual_key != *key {
                        event.properties.remove(&actual_key);
                        event.properties.insert(key.clone(), value.clone());
                    }
                    found.push((key.clone(), s));
                }
            }
        }
    }

    if found.is_empty() {
        return;
    }

    // Merge into the canonical $attribution object without clobbering values
    // a previous stage may already have placed there.
    let entry = event
        .properties
        .entry(ATTRIBUTION_PROPERTY.to_owned())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let map = match entry {
        Value::Object(map) => map,
        other => {
            // A non-object $attribution would shadow preserved IDs; replace it
            // rather than silently losing data.
            *other = Value::Object(serde_json::Map::new());
            match other {
                Value::Object(map) => map,
                _ => unreachable!("just assigned an object"),
            }
        }
    };

    for (key, value) in found {
        match map.get(&key) {
            Some(Value::String(existing)) if !existing.is_empty() => {}
            _ => {
                map.insert(key.clone(), Value::String(value));
                report.click_ids_preserved += 1;
                report.preserved.push(key);
            }
        }
    }
}

fn click_id_consent_ok(consent: &ConsentSnapshot) -> bool {
    if consent.purposes.contains_key("advertising") {
        consent.allows(Some("advertising"))
    } else {
        consent.any_granted()
    }
}

/// Render a scalar JSON value as a string; objects and arrays return `None`.
fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConsentConfig, IngestConfig};
    use crate::consent::resolve as resolve_consent;
    use chrono::Utc;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn event_with(props: Value) -> Event {
        Event::parse(
            &json!({"event": "page_view", "distinct_id": "u1", "properties": props}),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap()
    }

    fn consent(granted: bool) -> ConsentSnapshot {
        let cfg = ConsentConfig::default();
        let purposes: BTreeMap<String, bool> =
            cfg.purposes.iter().map(|p| (p.clone(), granted)).collect();
        resolve_consent(Some(&purposes), None, &cfg)
    }

    fn enabled() -> AttributionConfig {
        AttributionConfig {
            preserve_click_ids: true,
            ..AttributionConfig::default()
        }
    }

    #[test]
    fn disabled_config_is_a_noop() {
        let mut e = event_with(json!({"gclid": "abc"}));
        let mut r = AttributionReport::default();
        preserve(&mut e, &AttributionConfig::default(), &consent(true), &mut r);
        assert!(!e.properties.contains_key(ATTRIBUTION_PROPERTY));
        assert_eq!(r.click_ids_preserved, 0);
    }

    #[test]
    fn preserves_recognised_click_ids() {
        let mut e = event_with(json!({"gclid": "g-1", "fbclid": "f-2", "url": "/p"}));
        let mut r = AttributionReport::default();
        preserve(&mut e, &enabled(), &consent(true), &mut r);
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["gclid"], "g-1");
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["fbclid"], "f-2");
        assert_eq!(r.click_ids_preserved, 2);
        assert!(r.preserved.contains(&"gclid".to_string()));
        assert!(r.preserved.contains(&"fbclid".to_string()));
    }

    #[test]
    fn normalises_key_case() {
        let mut e = event_with(json!({"GCLID": "g-1"}));
        let mut r = AttributionReport::default();
        preserve(&mut e, &enabled(), &consent(true), &mut r);
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["gclid"], "g-1");
        // The non-canonical original key is folded into the canonical one.
        assert!(!e.properties.contains_key("GCLID"));
        assert_eq!(e.properties["gclid"], "g-1");
    }

    #[test]
    fn custom_keys_are_honoured() {
        let mut e = event_with(json!({"my_click": "v"}));
        let cfg = AttributionConfig {
            preserve_click_ids: true,
            click_id_keys: vec!["MY_CLICK".into()],
            ..AttributionConfig::default()
        };
        let mut r = AttributionReport::default();
        preserve(&mut e, &cfg, &consent(true), &mut r);
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["my_click"], "v");
    }

    #[test]
    fn ignores_non_scalar_and_empty_values() {
        let mut e = event_with(json!({"gclid": {"nested": true}, "fbclid": ""}));
        let mut r = AttributionReport::default();
        preserve(&mut e, &enabled(), &consent(true), &mut r);
        assert_eq!(r.click_ids_preserved, 0);
        assert!(!e.properties.contains_key(ATTRIBUTION_PROPERTY));
    }

    #[test]
    fn consent_gating_skips_preservation() {
        let mut e = event_with(json!({"gclid": "g-1"}));
        let cfg = AttributionConfig {
            preserve_click_ids: true,
            require_consent: true,
            ..AttributionConfig::default()
        };
        let mut r = AttributionReport::default();
        preserve(&mut e, &cfg, &consent(false), &mut r);
        assert!(r.skipped_for_consent);
        assert_eq!(r.click_ids_preserved, 0);
        assert!(!e.properties.contains_key(ATTRIBUTION_PROPERTY));

        // With consent granted, it runs.
        let mut r = AttributionReport::default();
        preserve(&mut e, &cfg, &consent(true), &mut r);
        assert!(!r.skipped_for_consent);
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["gclid"], "g-1");
    }

    #[test]
    fn existing_attribution_values_are_not_clobbered() {
        let mut e = event_with(json!({
            "$attribution": {"gclid": "prior"},
            "fbclid": "new"
        }));
        let mut r = AttributionReport::default();
        preserve(&mut e, &enabled(), &consent(true), &mut r);
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["gclid"], "prior");
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["fbclid"], "new");
        assert_eq!(r.click_ids_preserved, 1);
    }

    #[test]
    fn non_object_attribution_is_replaced() {
        let mut e = event_with(json!({"$attribution": "bogus", "gclid": "g-1"}));
        let mut r = AttributionReport::default();
        preserve(&mut e, &enabled(), &consent(true), &mut r);
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["gclid"], "g-1");
    }

    #[test]
    fn numeric_click_ids_are_stringified() {
        let mut e = event_with(json!({"gclid": 12345}));
        let mut r = AttributionReport::default();
        preserve(&mut e, &enabled(), &consent(true), &mut r);
        assert_eq!(e.properties[ATTRIBUTION_PROPERTY]["gclid"], "12345");
    }
}
