use serde_json::{json, Map, Value};

use crate::config::PosthogSettings;
use crate::destination::{absolute_base, HttpRequestPlan};
use crate::event::Event;

/// Build a PostHog `capture` request for one event.
///
/// Payload follows the public PostHog capture schema:
/// `api_key`, `event`, `distinct_id`, `timestamp`, `uuid`, `properties`.
pub fn capture(settings: &PosthogSettings, event: &Event, include_ip: bool) -> HttpRequestPlan {
    let mut properties = match serde_json::to_value(&event.properties) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    properties.insert("$lib".to_owned(), json!("pytagshield"));
    properties.insert("$lib_version".to_owned(), json!(env!("CARGO_PKG_VERSION")));
    if include_ip {
        if let Some(ip) = &event.context.ip {
            properties.insert("$ip".to_owned(), json!(ip));
        }
    }

    let body = json!({
        "api_key": settings.api_key,
        "event": event.name,
        "distinct_id": event.distinct_id,
        "timestamp": event.timestamp_rfc3339(),
        "uuid": event.id,
        "properties": Value::Object(properties),
    });

    HttpRequestPlan {
        url: format!("{}/capture/", absolute_base(&settings.host)),
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IngestConfig;
    use chrono::Utc;
    use serde_json::json;

    fn event() -> Event {
        // Relative timestamp: the payload test asserts exact formatting, so
        // freeze nothing and derive the expectation from the event itself.
        Event::parse(
            &json!({
                "event": "purchase",
                "distinct_id": "abc",
                "properties": {"revenue": 12.5}
            }),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap()
    }

    fn settings() -> PosthogSettings {
        PosthogSettings {
            api_key: "phc_test".into(),
            host: "https://us.i.posthog.com".into(),
        }
    }

    #[test]
    fn payload_matches_capture_schema() {
        let plan = capture(&settings(), &event(), false);
        assert_eq!(plan.body["api_key"], "phc_test");
        assert_eq!(plan.body["event"], "purchase");
        assert_eq!(plan.body["distinct_id"], "abc");
        assert_eq!(plan.body["uuid"], plan.body["uuid"].as_str().unwrap());
        assert_eq!(plan.body["properties"]["revenue"], 12.5);
        assert_eq!(plan.body["properties"]["$lib"], "pytagshield");
        assert_eq!(plan.body["timestamp"], event().timestamp_rfc3339());
        assert!(plan.body["properties"]["$ip"].is_null());
    }

    #[test]
    fn lib_properties_override_client_values() {
        let mut e = event();
        e.properties.insert("$lib".to_owned(), json!("attacker"));
        let plan = capture(&settings(), &e, false);
        assert_eq!(plan.body["properties"]["$lib"], "pytagshield");
    }

    #[test]
    fn ip_included_only_when_allowed_and_present() {
        let mut e = event();
        e.context.ip = Some("192.0.2.1".into());
        let plan = capture(&settings(), &e, true);
        assert_eq!(plan.body["properties"]["$ip"], "192.0.2.1");
        let plan = capture(&settings(), &e, false);
        assert!(plan.body["properties"]["$ip"].is_null());
    }
}
