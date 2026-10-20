use serde::Serialize;
use serde_json::Value;

use crate::config::{Action, TransformRule};
use crate::consent::ConsentSnapshot;
use crate::event::Event;
use crate::route::matches;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TransformReport {
    /// Rules whose `when` clause accepted the event.
    pub rules_applied: usize,
    /// Actions actually executed.
    pub actions_applied: usize,
    /// Enrichment actions skipped because consent granted no purpose.
    pub enrichment_skipped: usize,
}

/// Apply configured transformations in declaration order.
///
/// Reductive actions (`rename`, `drop`, `map`) always run. Enrichment
/// (`set`) runs only when consent granted at least one purpose, so consent
/// enforcement always precedes enrichment.
pub fn apply(
    event: &mut Event,
    rules: &[TransformRule],
    consent: &ConsentSnapshot,
    report: &mut TransformReport,
) {
    let enrichment_allowed = consent.any_granted();

    for rule in rules {
        if let Some(when) = &rule.when {
            if !matches(when, event) {
                continue;
            }
        }
        report.rules_applied += 1;

        for action in &rule.actions {
            let is_enrichment = matches!(action, Action::Set { .. });
            if is_enrichment && !enrichment_allowed {
                report.enrichment_skipped += 1;
                continue;
            }
            apply_action(event, action);
            report.actions_applied += 1;
        }
    }
}

fn apply_action(event: &mut Event, action: &Action) {
    match action {
        Action::Rename { from, to } => {
            if let Some(value) = event.properties.remove(from) {
                event.properties.insert(to.clone(), value);
            }
        }
        Action::Drop { keys } => {
            for key in keys {
                event.properties.remove(key);
            }
        }
        Action::Set { key, value } => {
            event.properties.insert(key.clone(), value.clone());
        }
        Action::Map { key, from } => {
            if let Some(current) = event.properties.get(key) {
                let lookup = match current {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    other => other.to_string(),
                };
                if let Some(mapped) = from.get(&lookup) {
                    event.properties.insert(key.clone(), mapped.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConsentConfig, IngestConfig, MatchRule};
    use crate::consent::resolve as resolve_consent;
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn event_with(props: &[(&str, Value)]) -> Event {
        let mut raw = serde_json::Map::new();
        raw.insert("event".into(), Value::from("page_view"));
        raw.insert("distinct_id".into(), Value::from("u1"));
        let mut p = serde_json::Map::new();
        for (k, v) in props {
            p.insert(k.to_string(), v.clone());
        }
        raw.insert("properties".into(), Value::Object(p));
        Event::parse(&Value::Object(raw), &IngestConfig::default(), Utc::now()).unwrap()
    }

    fn consent(granted: bool) -> ConsentSnapshot {
        let cfg = ConsentConfig::default();
        let c: BTreeMap<String, bool> = cfg
            .purposes
            .iter()
            .map(|p| (p.clone(), granted))
            .collect();
        resolve_consent(Some(&c), None, &cfg)
    }

    #[test]
    fn rename_drop_set_map() {
        let mut e = event_with(&[
            ("url", Value::from("/old")),
            ("debug", Value::from(true)),
            ("status", Value::from(404)),
        ]);
        let rules: Vec<TransformRule> = serde_yaml::from_str(
            r#"
- actions:
    - action: rename
      from: url
      to: page_url
    - action: drop
      keys: [debug]
    - action: set
      key: source
      value: server
    - action: map
      key: status
      from:
        "404": missing
        "200": ok
"#,
        )
        .unwrap();

        let mut report = TransformReport::default();
        apply(&mut e, &rules, &consent(true), &mut report);

        assert_eq!(e.properties.get("page_url"), Some(&Value::from("/old")));
        assert!(!e.properties.contains_key("url"));
        assert!(!e.properties.contains_key("debug"));
        assert_eq!(e.properties.get("source"), Some(&Value::from("server")));
        assert_eq!(e.properties.get("status"), Some(&Value::from("missing")));
        assert_eq!(report.rules_applied, 1);
        assert_eq!(report.actions_applied, 4);
        assert_eq!(report.enrichment_skipped, 0);
    }

    #[test]
    fn enrichment_is_gated_on_consent() {
        let mut e = event_with(&[("url", Value::from("/old"))]);
        let rules: Vec<TransformRule> = serde_yaml::from_str(
            r#"
- actions:
    - action: rename
      from: url
      to: page_url
    - action: set
      key: source
      value: server
"#,
        )
        .unwrap();

        let mut report = TransformReport::default();
        apply(&mut e, &rules, &consent(false), &mut report);

        // Reductive action still ran; enrichment did not.
        assert!(e.properties.contains_key("page_url"));
        assert!(!e.properties.contains_key("source"));
        assert_eq!(report.actions_applied, 1);
        assert_eq!(report.enrichment_skipped, 1);
    }

    #[test]
    fn when_clause_gates_rule() {
        let mut e = event_with(&[("url", Value::from("/old"))]);
        let rules: Vec<TransformRule> = serde_yaml::from_str(
            r#"
- when:
    events: [purchase]
  actions:
    - action: rename
      from: url
      to: page_url
"#,
        )
        .unwrap();

        let mut report = TransformReport::default();
        apply(&mut e, &rules, &consent(true), &mut report);
        assert!(e.properties.contains_key("url"));
        assert_eq!(report.rules_applied, 0);
    }

    #[test]
    fn unmatched_map_value_is_kept() {
        let mut e = event_with(&[("status", Value::from(500))]);
        let rules: Vec<TransformRule> = serde_yaml::from_str(
            r#"
- actions:
    - action: map
      key: status
      from:
        "404": missing
"#,
        )
        .unwrap();
        let mut report = TransformReport::default();
        apply(&mut e, &rules, &consent(true), &mut report);
        assert_eq!(e.properties.get("status"), Some(&Value::from(500)));
    }

    #[test]
    fn rules_apply_in_order() {
        let mut e = event_with(&[("a", Value::from(1))]);
        let rules: Vec<TransformRule> = serde_yaml::from_str(
            r#"
- actions:
    - action: rename
      from: a
      to: b
- actions:
    - action: rename
      from: b
      to: c
"#,
        )
        .unwrap();
        let mut report = TransformReport::default();
        apply(&mut e, &rules, &consent(true), &mut report);
        assert_eq!(e.properties.get("c"), Some(&Value::from(1)));
        assert_eq!(report.rules_applied, 2);
    }

    #[test]
    fn when_matches_on_properties() {
        let mut e = event_with(&[("env", Value::from("prod"))]);
        let rules: Vec<TransformRule> = serde_yaml::from_str(
            r#"
- when:
    events: ["*"]
    properties:
      env: [prod]
  actions:
    - action: set
      key: scoped
      value: true
"#,
        )
        .unwrap();
        let mut report = TransformReport::default();
        apply(&mut e, &rules, &consent(true), &mut report);
        assert_eq!(e.properties.get("scoped"), Some(&Value::from(true)));
        assert_eq!(report.rules_applied, 1);
    }

    #[test]
    fn match_rule_serde_roundtrip() {
        // MatchRule is used both for routes and transforms; keep it strict.
        let rule: MatchRule = serde_yaml::from_str("events: [a]\nproperties:\n  k: [1]").unwrap();
        assert_eq!(rule.events, vec!["a"]);
        assert!(serde_yaml::from_str::<MatchRule>("bogus: 1").is_err());
    }
}
