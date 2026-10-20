use serde_json::{json, Map, Value};

use crate::config::SgtmSettings;
use crate::destination::HttpRequestPlan;
use crate::event::Event;

/// Build a server-side GTM compatible `collect` request for one event.
///
/// The payload mirrors the GA4 client shape a server GTM container expects
/// from a web container, so an existing sGTM setup can consume PyTagShield as
/// its upstream producer:
/// `{ client_id, events: [ { name, params } ], [user_properties] }`.
///
/// Unlike the GA4 destination, credentials are not embedded by default;
/// operators add any required query parameters via `sgtm.query`.
pub fn collect(settings: &SgtmSettings, event: &Event, include_ip: bool) -> HttpRequestPlan {
    let mut params = match serde_json::to_value(&event.properties) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    // Carry the event identity and time so downstream GTM tags can rebuild
    // the full event without extra lookups.
    params.insert("event_id".to_owned(), json!(event.id));
    params.insert("timestamp_millis".to_owned(), json!(event.timestamp_millis));
    if include_ip {
        if let Some(ip) = &event.context.ip {
            params.insert("ip_override".to_owned(), json!(ip));
        }
    }
    if let Some(ua) = &event.context.user_agent {
        params.insert("user_agent".to_owned(), json!(ua));
    }

    let body = json!({
        "client_id": event.distinct_id,
        "events": [
            {
                "name": event.name,
                "params": Value::Object(params),
            }
        ],
    });

    HttpRequestPlan {
        url: build_url(settings),
        headers: build_headers(settings),
        body,
    }
}

fn build_url(settings: &SgtmSettings) -> String {
    let mut url = settings.url.clone();
    let mut pairs: Vec<String> = Vec::new();
    if let Some(mid) = &settings.measurement_id {
        if !mid.trim().is_empty() {
            pairs.push(format!("measurement_id={}", urlencode(mid)));
        }
    }
    // Deterministic ordering for testability.
    let mut query: Vec<(&String, &String)> = settings.query.iter().collect();
    query.sort();
    for (k, v) in query {
        pairs.push(format!("{}={}", urlencode(k), urlencode(v)));
    }
    if !pairs.is_empty() {
        let sep = if url.contains('?') { '&' } else { '?' };
        url.push(sep);
        url.push_str(&pairs.join("&"));
    }
    url
}

fn build_headers(settings: &SgtmSettings) -> Vec<(String, String)> {
    let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
    for (k, v) in &settings.headers {
        if k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        headers.push((k.clone(), v.clone()));
    }
    headers
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IngestConfig;
    use chrono::Utc;
    use std::collections::HashMap;

    fn event() -> Event {
        Event::parse(
            &json!({"event":"purchase","distinct_id":"u1","properties":{"value":9}}),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap()
    }

    fn settings() -> SgtmSettings {
        SgtmSettings {
            url: "https://sgtm.example.com/g/collect".into(),
            measurement_id: None,
            query: HashMap::new(),
            headers: HashMap::new(),
        }
    }

    #[test]
    fn payload_matches_ga4_client_shape() {
        let plan = collect(&settings(), &event(), false);
        assert_eq!(plan.url, "https://sgtm.example.com/g/collect");
        assert_eq!(plan.body["client_id"], "u1");
        assert_eq!(plan.body["events"][0]["name"], "purchase");
        assert_eq!(plan.body["events"][0]["params"]["value"], 9);
        assert!(plan.body["events"][0]["params"]["event_id"].is_string());
    }

    #[test]
    fn measurement_id_and_query_are_appended() {
        let mut s = settings();
        s.measurement_id = Some("G-XYZ".into());
        s.query = [("z".to_string(), "2".to_string()), ("a".to_string(), "1".to_string())].into();
        let plan = collect(&s, &event(), false);
        // Deterministic ordering: measurement_id first, then sorted query.
        assert_eq!(
            plan.url,
            "https://sgtm.example.com/g/collect?measurement_id=G-XYZ&a=1&z=2"
        );
    }

    #[test]
    fn existing_query_string_is_extended() {
        let mut s = settings();
        s.url = "https://sgtm.example.com/g/collect?tid=1".into();
        s.query = [("a".to_string(), "1".to_string())].into();
        let plan = collect(&s, &event(), false);
        assert_eq!(plan.url, "https://sgtm.example.com/g/collect?tid=1&a=1");
    }

    #[test]
    fn headers_forwarded_and_content_type_protected() {
        let mut s = settings();
        s.headers = [
            ("X-Auth".to_string(), "t".to_string()),
            ("Content-Type".to_string(), "text/plain".to_string()),
        ]
        .into();
        let plan = collect(&s, &event(), false);
        assert!(plan.headers.contains(&("X-Auth".to_string(), "t".to_string())));
        let cts: Vec<_> = plan
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .collect();
        assert_eq!(cts.len(), 1);
        assert_eq!(cts[0].1, "application/json");
    }

    #[test]
    fn context_forwarded_and_ip_gated() {
        let mut e = event();
        e.context.ip = Some("198.51.100.1".into());
        e.context.user_agent = Some("UA".into());
        let plan = collect(&settings(), &e, true);
        assert_eq!(plan.body["events"][0]["params"]["ip_override"], "198.51.100.1");
        assert_eq!(plan.body["events"][0]["params"]["user_agent"], "UA");
        let plan = collect(&settings(), &e, false);
        assert!(plan.body["events"][0]["params"]["ip_override"].is_null());
    }
}
