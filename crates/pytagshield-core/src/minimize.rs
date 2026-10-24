use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::MinimizationConfig;
use crate::event::Event;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MinimizationReport {
    /// Properties removed by the drop list, allowlist, property cap or
    /// depth cap.
    pub properties_dropped: usize,
    /// String values truncated to the configured maximum length.
    pub values_truncated: usize,
    /// distinct_id was replaced by its salted hash.
    pub id_pseudonymized: bool,
    /// Client IP cleared from the event context.
    pub ip_cleared: bool,
}

/// Enforce the final minimization gate on an event.
///
/// This runs after consent resolution and after transformations so that no
/// configured transform can reintroduce data that minimization forbids.
/// Every removal is counted and surfaced in the report and metrics.
pub fn minimize(event: &mut Event, cfg: &MinimizationConfig, report: &mut MinimizationReport) {
    // 1. Unconditional drop list (case-insensitive).
    if !cfg.drop_properties.is_empty() {
        let drops: Vec<String> = event
            .properties
            .keys()
            .filter(|k| {
                let lower = k.to_ascii_lowercase();
                cfg.drop_properties
                    .iter()
                    .any(|d| d.to_ascii_lowercase() == lower)
            })
            .cloned()
            .collect();
        for k in drops {
            event.properties.remove(&k);
            report.properties_dropped += 1;
        }
    }

    // 2. Allowlist (case-insensitive) when configured.
    if !cfg.allowlist_properties.is_empty() {
        let allowed_lower: Vec<String> = cfg
            .allowlist_properties
            .iter()
            .map(|k| k.to_ascii_lowercase())
            .collect();
        let keep: Vec<String> = event
            .properties
            .keys()
            .filter(|k| allowed_lower.contains(&k.to_ascii_lowercase()))
            .cloned()
            .collect();
        let remove: Vec<String> = event
            .properties
            .keys()
            .filter(|k| !keep.iter().any(|kk| kk == *k))
            .cloned()
            .collect();
        for k in remove {
            event.properties.remove(&k);
            report.properties_dropped += 1;
        }
    }

    // 3. Property cap; excess keys dropped in deterministic (alphabetical)
    //    order so behaviour is reproducible across runs.
    if event.properties.len() > cfg.max_properties {
        let excess = event.properties.len() - cfg.max_properties;
        let victims: Vec<String> = event
            .properties
            .keys()
            .skip(cfg.max_properties)
            .take(excess)
            .cloned()
            .collect();
        for k in victims {
            event.properties.remove(&k);
            report.properties_dropped += 1;
        }
    }

    // 4. Depth cap: drop values nested beyond the limit.
    let deep: Vec<String> = event
        .properties
        .iter()
        .filter(|(_, v)| value_depth(v) > cfg.max_nesting_depth)
        .map(|(k, _)| k.clone())
        .collect();
    for k in deep {
        event.properties.remove(&k);
        report.properties_dropped += 1;
    }

    // 5. String truncation (recursive, char-boundary safe).
    for value in event.properties.values_mut() {
        truncate_strings(
            value,
            cfg.max_value_string_len,
            &mut report.values_truncated,
        );
    }

    // 6. Pseudonymize the outbound identifier. The raw identifier was
    //    already used (in memory only) for consent lookup.
    if cfg.pseudonymize_ids && !cfg.pseudonymize_salt.is_empty() {
        event.distinct_id = salted_hash(&cfg.pseudonymize_salt, &event.distinct_id);
        report.id_pseudonymized = true;
    }

    // 7. Client IP never leaves the process unless explicitly enabled.
    if !cfg.forward_ip && event.context.ip.is_some() {
        event.context.ip = None;
        report.ip_cleared = true;
    }

    // 8. Bound the user agent string.
    if let Some(ua) = &mut event.context.user_agent {
        if ua.chars().count() > 256 {
            let truncated: String = ua.chars().take(256).collect();
            *ua = truncated;
            report.values_truncated += 1;
        }
    }
}

/// Salted SHA-256 pseudonym. The `sha256:` prefix keeps the format
/// self-describing for downstream consumers.
pub fn salted_hash(salt: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(value.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn value_depth(v: &Value) -> usize {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => 1,
        Value::Array(items) => 1 + items.iter().map(value_depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(value_depth).max().unwrap_or(0),
    }
}

fn truncate_strings(v: &mut Value, max_len: usize, counter: &mut usize) {
    match v {
        Value::String(s) => {
            if s.chars().count() > max_len {
                *s = s.chars().take(max_len).collect();
                *counter += 1;
            }
        }
        Value::Array(items) => {
            for item in items {
                truncate_strings(item, max_len, counter);
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                truncate_strings(value, max_len, counter);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IngestConfig;
    use chrono::Utc;
    use serde_json::json;

    fn event_with(props: serde_json::Value) -> Event {
        let mut raw = json!({
            "event": "page_view",
            "distinct_id": "user@example.com",
            "properties": props,
        });
        raw["properties"] = props;
        let mut e = Event::parse(&raw, &IngestConfig::default(), Utc::now()).unwrap();
        e.context.ip = Some("203.0.113.9".into());
        e.context.user_agent = Some("Mozilla/5.0".into());
        e
    }

    fn cfg() -> MinimizationConfig {
        MinimizationConfig {
            pseudonymize_salt: "testsalt".into(),
            ..MinimizationConfig::default()
        }
    }

    #[test]
    fn drops_pii_properties_case_insensitively() {
        let mut e = event_with(json!({"Email": "a@b.c", "phone": "123", "page": "/"}));
        let mut r = MinimizationReport::default();
        minimize(&mut e, &cfg(), &mut r);
        assert!(!e.properties.contains_key("Email"));
        assert!(!e.properties.contains_key("phone"));
        assert!(e.properties.contains_key("page"));
        assert_eq!(r.properties_dropped, 2);
    }

    #[test]
    fn allowlist_keeps_only_listed_keys() {
        let mut e = event_with(json!({"page": "/", "debug": "x", "keep": 1}));
        let c = MinimizationConfig {
            allowlist_properties: vec!["page".into(), "keep".into()],
            ..cfg()
        };
        let mut r = MinimizationReport::default();
        minimize(&mut e, &c, &mut r);
        assert_eq!(
            e.properties.keys().collect::<Vec<_>>(),
            vec!["keep", "page"]
        );
        assert_eq!(r.properties_dropped, 1);
    }

    #[test]
    fn property_cap_drops_deterministically() {
        let mut props = serde_json::Map::new();
        for k in ["b", "a", "c", "d"] {
            props.insert(k.to_string(), json!(1));
        }
        let mut e = event_with(Value::Object(props));
        let c = MinimizationConfig {
            max_properties: 2,
            ..cfg()
        };
        let mut r = MinimizationReport::default();
        minimize(&mut e, &c, &mut r);
        // Alphabetically first two keys survive.
        assert_eq!(e.properties.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(r.properties_dropped, 2);
    }

    #[test]
    fn truncates_long_strings_recursively() {
        let long = "x".repeat(100);
        let mut e = event_with(json!({"s": long, "nested": {"s": long}}));
        let c = MinimizationConfig {
            max_value_string_len: 10,
            ..cfg()
        };
        let mut r = MinimizationReport::default();
        minimize(&mut e, &c, &mut r);
        assert_eq!(e.properties["s"].as_str().unwrap().len(), 10);
        assert_eq!(e.properties["nested"]["s"].as_str().unwrap().len(), 10);
        assert_eq!(r.values_truncated, 2);
    }

    #[test]
    fn drops_overly_deep_values() {
        let deep = json!({"l1": {"l2": {"l3": {"l4": "v"}}}});
        let mut e = event_with(json!({"deep": deep, "flat": 1}));
        let c = MinimizationConfig {
            max_nesting_depth: 3,
            ..cfg()
        };
        let mut r = MinimizationReport::default();
        minimize(&mut e, &c, &mut r);
        assert!(!e.properties.contains_key("deep"));
        assert!(e.properties.contains_key("flat"));
        assert_eq!(r.properties_dropped, 1);
    }

    #[test]
    fn pseudonymizes_distinct_id_deterministically() {
        let mut e = event_with(json!({}));
        let mut r = MinimizationReport::default();
        minimize(&mut e, &cfg(), &mut r);
        let hashed = e.distinct_id.clone();
        assert!(hashed.starts_with("sha256:"));
        assert_ne!(hashed, "user@example.com");
        assert!(r.id_pseudonymized);

        // Same salt + input => same pseudonym; different salt => different.
        let again = salted_hash("testsalt", "user@example.com");
        assert_eq!(hashed, again);
        assert_ne!(salted_hash("othersalt", "user@example.com"), again);
    }

    #[test]
    fn clears_ip_unless_forwarding_enabled() {
        let mut e = event_with(json!({}));
        let mut r = MinimizationReport::default();
        minimize(&mut e, &cfg(), &mut r);
        assert!(e.context.ip.is_none());
        assert!(r.ip_cleared);

        let mut e = event_with(json!({}));
        let c = MinimizationConfig {
            forward_ip: true,
            ..cfg()
        };
        let mut r = MinimizationReport::default();
        minimize(&mut e, &c, &mut r);
        assert_eq!(e.context.ip.as_deref(), Some("203.0.113.9"));
        assert!(!r.ip_cleared);
    }

    #[test]
    fn truncates_user_agent() {
        let mut e = event_with(json!({}));
        e.context.user_agent = Some("u".repeat(1000));
        let mut r = MinimizationReport::default();
        minimize(&mut e, &cfg(), &mut r);
        assert_eq!(e.context.user_agent.as_ref().unwrap().chars().count(), 256);
    }
}
