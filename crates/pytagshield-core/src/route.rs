use serde::Serialize;

use crate::config::{DestinationConfig, MatchRule, RouteConfig};
use crate::consent::ConsentSnapshot;
use crate::event::Event;

/// Wildcard matcher supporting `*` (any run) and `?` (single character).
/// Iterative backtracking; linear in the common case.
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let val: Vec<char> = value.chars().collect();
    let (mut p, mut v) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);

    while v < val.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == val[v]) {
            p += 1;
            v += 1;
        } else if p < pat.len() && pat[p] == '*' {
            star = Some(p);
            p += 1;
            mark = v;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            v = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

/// Evaluate a match rule against an event. All conditions must hold.
pub fn matches(rule: &MatchRule, event: &Event) -> bool {
    if !rule.events.iter().any(|p| pattern_matches(p, &event.name)) {
        return false;
    }
    for (key, allowed) in &rule.properties {
        let actual = event.properties.get(key);
        let matched = match actual {
            Some(v) => allowed.iter().any(|a| a == v),
            None => allowed.iter().any(|a| a.is_null()),
        };
        if !matched {
            return false;
        }
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkippedDestination {
    pub destination: String,
    pub reason: String,
}

/// Result of evaluating all routes for one event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RoutingOutcome {
    /// Names of enabled routes whose match rule accepted the event.
    pub matched_routes: Vec<String>,
    /// Destination names to deliver to, in configuration order, deduplicated.
    pub destinations: Vec<String>,
    /// Destinations that a matched route selected but that were not used.
    pub skipped: Vec<SkippedDestination>,
}

/// Evaluate routes and apply consent gating for one event.
///
/// A destination is dispatched only when:
/// 1. at least one enabled route matched the event and names it, and
/// 2. the destination is enabled, and
/// 3. the consent snapshot permits the destination's required purpose.
pub fn resolve(
    event: &Event,
    routes: &[RouteConfig],
    destinations: &[DestinationConfig],
    consent: &ConsentSnapshot,
) -> RoutingOutcome {
    let mut outcome = RoutingOutcome::default();

    for route in routes {
        if !route.enabled {
            continue;
        }
        if matches(&route.rule, event) {
            outcome.matched_routes.push(route.name.clone());
        }
    }

    if outcome.matched_routes.is_empty() {
        return outcome;
    }

    let selected: Vec<&str> = routes
        .iter()
        .filter(|r| r.enabled && outcome.matched_routes.iter().any(|n| n == &r.name))
        .flat_map(|r| r.destinations.iter().map(|d| d.as_str()))
        .collect();

    for dest in destinations {
        if !selected.contains(&dest.name.as_str()) {
            continue;
        }
        if !dest.enabled {
            outcome.skipped.push(SkippedDestination {
                destination: dest.name.clone(),
                reason: "destination_disabled".to_owned(),
            });
            continue;
        }
        if !consent.allows(dest.purpose.as_deref()) {
            outcome.skipped.push(SkippedDestination {
                destination: dest.name.clone(),
                reason: format!(
                    "consent_denied:{}",
                    dest.purpose.as_deref().unwrap_or("unknown")
                ),
            });
            continue;
        }
        if !outcome.destinations.contains(&dest.name) {
            outcome.destinations.push(dest.name.clone());
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConsentConfig, DestinationType, IngestConfig};
    use crate::consent::resolve as resolve_consent;
    use chrono::Utc;
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn event(name: &str, props: &[(&str, Value)]) -> Event {
        let mut raw = serde_json::Map::new();
        raw.insert("event".into(), Value::from(name));
        raw.insert("distinct_id".into(), Value::from("u1"));
        let mut p = serde_json::Map::new();
        for (k, v) in props {
            p.insert(k.to_string(), v.clone());
        }
        raw.insert("properties".into(), Value::Object(p));
        Event::parse(&Value::Object(raw), &IngestConfig::default(), Utc::now()).unwrap()
    }

    #[test]
    fn wildcard_matching() {
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("page_*", "page_view"));
        assert!(!pattern_matches("page_*", "xpage_view"));
        assert!(pattern_matches("*_view", "page_view"));
        assert!(pattern_matches("*view*", "page_viewed"));
        assert!(pattern_matches("exact", "exact"));
        assert!(!pattern_matches("exact", "exactly"));
        assert!(pattern_matches("a?c", "abc"));
        assert!(!pattern_matches("a?c", "ac"));
        assert!(pattern_matches("**", "anything"));
        assert!(pattern_matches("a*b*c", "axxbyyc"));
        assert!(!pattern_matches("a*b*c", "axxbyy"));
        assert!(pattern_matches("", ""));
        assert!(!pattern_matches("", "x"));
    }

    #[test]
    fn match_rule_requires_all_conditions() {
        let e = event(
            "page_view",
            &[("env", Value::from("prod")), ("v", Value::from(2))],
        );
        let rule = MatchRule {
            events: vec!["page_*".into()],
            properties: BTreeMap::from([("env".to_string(), vec![Value::from("prod")])]),
        };
        assert!(matches(&rule, &e));

        let rule = MatchRule {
            events: vec!["page_*".into()],
            properties: BTreeMap::from([("env".to_string(), vec![Value::from("staging")])]),
        };
        assert!(!matches(&rule, &e));
    }

    #[test]
    fn missing_property_matches_only_when_null_listed() {
        let e = event("e", &[]);
        let mut props = BTreeMap::new();
        props.insert("absent".to_string(), vec![Value::Null]);
        let rule = MatchRule {
            events: vec!["*".into()],
            properties: props,
        };
        assert!(matches(&rule, &e));

        let rule = MatchRule {
            events: vec!["*".into()],
            properties: BTreeMap::from([("absent".to_string(), vec![Value::from("x")])]),
        };
        assert!(!matches(&rule, &e));
    }

    fn destinations() -> Vec<DestinationConfig> {
        vec![
            DestinationConfig {
                name: "analytics".into(),
                kind: DestinationType::Log,
                purpose: Some("analytics".into()),
                enabled: true,
                forward_ip: None,
                queue_capacity: None,
                posthog: None,
                ga4: None,
                meta: None,
                sgtm: None,
                webhook: None,
            },
            DestinationConfig {
                name: "ads".into(),
                kind: DestinationType::Log,
                purpose: Some("advertising".into()),
                enabled: true,
                forward_ip: None,
                queue_capacity: None,
                posthog: None,
                ga4: None,
                meta: None,
                sgtm: None,
                webhook: None,
            },
            DestinationConfig {
                name: "disabled".into(),
                kind: DestinationType::Log,
                purpose: None,
                enabled: false,
                forward_ip: None,
                queue_capacity: None,
                posthog: None,
                ga4: None,
                meta: None,
                sgtm: None,
                webhook: None,
            },
        ]
    }

    fn route(name: &str, dests: &[&str]) -> RouteConfig {
        RouteConfig {
            name: name.into(),
            destinations: dests.iter().map(|s| s.to_string()).collect(),
            rule: MatchRule::default(),
            enabled: true,
        }
    }

    fn consent_with(pairs: &[(&str, bool)]) -> ConsentSnapshot {
        let cfg = ConsentConfig::default();
        let c: BTreeMap<String, bool> = pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        resolve_consent(Some(&c), None, &cfg)
    }

    #[test]
    fn routes_and_gates_by_consent() {
        let e = event("page_view", &[]);
        let routes = vec![route("all", &["analytics", "ads", "disabled"])];
        let dests = destinations();

        let granted = consent_with(&[("analytics", true), ("advertising", false)]);
        let out = resolve(&e, &routes, &dests, &granted);
        assert_eq!(out.matched_routes, vec!["all"]);
        assert_eq!(out.destinations, vec!["analytics"]);
        assert_eq!(out.skipped.len(), 2);
        // Skips follow configuration order.
        assert_eq!(out.skipped[0].destination, "ads");
        assert_eq!(out.skipped[0].reason, "consent_denied:advertising");
        assert_eq!(out.skipped[1].destination, "disabled");
        assert_eq!(out.skipped[1].reason, "destination_disabled");
    }

    #[test]
    fn no_route_match_means_no_destinations() {
        let e = event("page_view", &[]);
        let routes = vec![RouteConfig {
            name: "checkout".into(),
            destinations: vec!["analytics".into()],
            rule: MatchRule {
                events: vec!["purchase".into()],
                properties: BTreeMap::new(),
            },
            enabled: true,
        }];
        let out = resolve(
            &e,
            &routes,
            &dests_empty(),
            &consent_with(&[("analytics", true)]),
        );
        assert!(out.destinations.is_empty());
        assert!(out.skipped.is_empty());
    }

    fn dests_empty() -> Vec<DestinationConfig> {
        destinations()
    }

    #[test]
    fn disabled_routes_are_ignored() {
        let e = event("page_view", &[]);
        let mut r = route("all", &["analytics"]);
        r.enabled = false;
        let out = resolve(
            &e,
            std::slice::from_ref(&r),
            &destinations(),
            &consent_with(&[("analytics", true)]),
        );
        assert!(out.matched_routes.is_empty());
        assert!(out.destinations.is_empty());
    }

    #[test]
    fn duplicates_are_deduplicated() {
        let e = event("page_view", &[]);
        let routes = vec![route("a", &["analytics"]), route("b", &["analytics"])];
        let out = resolve(
            &e,
            &routes,
            &destinations(),
            &consent_with(&[("analytics", true)]),
        );
        assert_eq!(out.destinations, vec!["analytics"]);
    }
}
