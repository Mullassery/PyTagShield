use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::WebhookSettings;
use crate::consent::{ConsentSnapshot, ConsentSource};
use crate::destination::HttpRequestPlan;
use crate::event::{Event, EventContext};

/// Versioned webhook envelope. The `schema` field lets consumers detect
/// breaking changes; new optional fields may be added within a schema version.
pub const ENVELOPE_SCHEMA: &str = "pytagshield.event.v1";
/// Versioned shape of the consent decision block carried on approved events.
pub const CONSENT_SCHEMA: &str = "pytagshield.consent.v1";

#[derive(Debug, Serialize)]
struct Envelope<'a> {
    schema: &'static str,
    event_id: &'a str,
    event: &'a str,
    distinct_id: &'a str,
    timestamp_millis: i64,
    timestamp: String,
    properties: &'a std::collections::BTreeMap<String, serde_json::Value>,
    /// Consent contract version for this event.
    consent_schema: &'static str,
    /// Purposes that were granted for this event (denied purposes are
    /// omitted entirely rather than sent as `false`).
    consent: BTreeMap<String, bool>,
    /// Full decision for every configured purpose, including denials.
    consent_decisions: BTreeMap<String, bool>,
    /// Where the effective decision came from: event, stored, or default.
    consent_source: ConsentSource,
    context: &'a EventContext,
}

/// Build a webhook request carrying the versioned envelope of an event that
/// has already passed consent enforcement and minimization.
pub fn request(
    settings: &WebhookSettings,
    event: &Event,
    consent: &ConsentSnapshot,
) -> HttpRequestPlan {
    let envelope = Envelope {
        schema: ENVELOPE_SCHEMA,
        event_id: &event.id,
        event: &event.name,
        distinct_id: &event.distinct_id,
        timestamp_millis: event.timestamp_millis,
        timestamp: event.timestamp_rfc3339(),
        properties: &event.properties,
        consent_schema: CONSENT_SCHEMA,
        consent: consent.granted(),
        consent_decisions: consent.purposes.clone(),
        consent_source: consent.source,
        context: &event.context,
    };

    let body = match serde_json::to_value(&envelope) {
        Ok(v) => v,
        Err(e) => serde_json::json!({
            "schema": ENVELOPE_SCHEMA,
            "error": format!("envelope serialization failed: {e}"),
        }),
    };

    let mut headers: Vec<(String, String)> =
        vec![("content-type".to_owned(), "application/json".to_owned())];
    for (k, v) in &settings.headers {
        // Content-type stays governed by the JSON body.
        if k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        headers.push((k.clone(), v.clone()));
    }

    HttpRequestPlan {
        url: settings.url.clone(),
        headers,
        body,
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

    fn consent(granted: bool) -> ConsentSnapshot {
        let cfg = ConsentConfig::default();
        let purposes: BTreeMap<String, bool> =
            cfg.purposes.iter().map(|p| (p.clone(), granted)).collect();
        resolve_consent(Some(&purposes), None, &cfg)
    }

    #[test]
    fn envelope_is_versioned_and_carries_the_event() {
        let mut e = Event::parse(
            &json!({"event":"e","distinct_id":"u","properties":{"a":1}}),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap();
        e.context.user_agent = Some("UA".into());

        let settings = WebhookSettings {
            url: "https://example.com/hook".into(),
            headers: Default::default(),
        };
        let plan = request(&settings, &e, &consent(true));
        assert_eq!(plan.body["schema"], ENVELOPE_SCHEMA);
        assert_eq!(plan.body["event"], "e");
        assert_eq!(plan.body["event_id"], e.id);
        assert_eq!(plan.body["distinct_id"], "u");
        assert_eq!(plan.body["properties"]["a"], 1);
        assert_eq!(plan.body["timestamp_millis"], e.timestamp_millis);
        assert_eq!(plan.body["consent_schema"], CONSENT_SCHEMA);
        assert_eq!(plan.body["consent"]["analytics"], true);
        assert_eq!(plan.body["consent_decisions"]["analytics"], true);
        assert_eq!(plan.body["consent_source"], "event");
        assert_eq!(plan.body["context"]["user_agent"], "UA");
    }

    #[test]
    fn denied_purposes_are_omitted_from_consent_block() {
        let e = Event::parse(
            &json!({"event":"e","distinct_id":"u"}),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap();
        let settings = WebhookSettings {
            url: "https://example.com/hook".into(),
            headers: Default::default(),
        };
        let plan = request(&settings, &e, &consent(false));
        assert!(plan.body["consent"].as_object().unwrap().is_empty());
        assert_eq!(plan.body["consent_decisions"]["analytics"], false);
        assert_eq!(plan.body["consent_source"], "event");
    }

    #[test]
    fn custom_headers_are_forwarded_and_content_type_is_protected() {
        let mut e = Event::parse(
            &json!({"event":"e","distinct_id":"u"}),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap();
        e.context.user_agent = Some("UA".into());

        let settings = WebhookSettings {
            url: "https://example.com/hook".into(),
            headers: [
                ("X-Auth".to_string(), "secret".to_string()),
                ("Content-Type".to_string(), "text/plain".to_string()),
            ]
            .into(),
        };
        let plan = request(&settings, &e, &consent(true));
        assert_eq!(plan.url, "https://example.com/hook");
        assert!(plan
            .headers
            .contains(&("X-Auth".to_string(), "secret".to_string())));
        let content_types: Vec<_> = plan
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .collect();
        assert_eq!(content_types.len(), 1);
        assert_eq!(content_types[0].1, "application/json");
        assert_eq!(plan.body["event"], "e");
    }
}
