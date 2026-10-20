use serde_json::{json, Map, Value};

use crate::config::Ga4Settings;
use crate::destination::{absolute_base, HttpRequestPlan};
use crate::event::Event;

/// Build a GA4 Measurement Protocol `collect` request for one event.
///
/// The Measurement Protocol expects a single event per request with the
/// shape:
/// `{ client_id, [user_id], [events: [ { name, params } ]], [user_properties] }`.
/// Credentials travel in the query string (`measurement_id`, `api_secret`),
/// which is how the protocol is designed; the body is JSON.
///
/// See: <https://developers.google.com/analytics/devguides/collection/protocol/ga4>
pub fn collect(settings: &Ga4Settings, event: &Event, include_ip: bool) -> HttpRequestPlan {
    // GA4 requires a client_id. The pseudonymized distinct id is stable and
    // non-reversible, which is exactly what a client identifier should be.
    let client_id = event.distinct_id.clone();

    let mut params = match serde_json::to_value(&event.properties) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    params.insert("engagement_time_msec".to_owned(), json!(1));
    if settings.debug_mode {
        params.insert("debug_mode".to_owned(), json!(1));
    }
    if include_ip {
        if let Some(ip) = &event.context.ip {
            // GA4 drops ip_override by default; this is opt-in per destination
            // and only reaches the payload when global forwarding is enabled.
            params.insert("ip_override".to_owned(), json!(ip));
        }
    }
    if let Some(ua) = &event.context.user_agent {
        params.insert("user_agent".to_owned(), json!(ua));
    }

    let body = json!({
        "client_id": client_id,
        "timestamp_micros": event.timestamp_millis.saturating_mul(1000).to_string(),
        "events": [
            {
                "name": event.name,
                "params": Value::Object(params),
            }
        ],
    });

    // Credentials go in the query string; encode just enough to be safe.
    let url = format!(
        "{}/mp/collect?measurement_id={}&api_secret={}",
        absolute_base(&settings.host),
        urlencode(&settings.measurement_id),
        urlencode(&settings.api_secret),
    );

    HttpRequestPlan {
        url,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body,
    }
}

/// Minimal percent-encoding for query-string values (unreserved chars kept).
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

    fn event() -> Event {
        Event::parse(
            &json!({
                "event": "purchase",
                "distinct_id": "abc",
                "properties": {"currency": "USD", "value": 12.5}
            }),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap()
    }

    fn settings() -> Ga4Settings {
        Ga4Settings {
            measurement_id: "G-TEST123".into(),
            api_secret: "s3cret".into(),
            host: "https://www.google-analytics.com".into(),
            debug_mode: false,
        }
    }

    #[test]
    fn payload_matches_measurement_protocol_schema() {
        let plan = collect(&settings(), &event(), false);
        assert_eq!(
            plan.url,
            "https://www.google-analytics.com/mp/collect?measurement_id=G-TEST123&api_secret=s3cret"
        );
        assert_eq!(plan.body["client_id"], "abc");
        assert_eq!(plan.body["events"][0]["name"], "purchase");
        assert_eq!(plan.body["events"][0]["params"]["currency"], "USD");
        assert_eq!(plan.body["events"][0]["params"]["value"], 12.5);
        assert_eq!(plan.body["events"][0]["params"]["engagement_time_msec"], 1);
        assert!(plan.body["events"][0]["params"]["debug_mode"].is_null());
    }

    #[test]
    fn timestamp_is_micros_string() {
        let plan = collect(&settings(), &event(), false);
        let micros = plan.body["timestamp_micros"].as_str().unwrap();
        assert_eq!(micros.parse::<i64>().unwrap(), event().timestamp_millis * 1000);
    }

    #[test]
    fn debug_mode_is_opt_in() {
        let mut s = settings();
        s.debug_mode = true;
        let plan = collect(&s, &event(), false);
        assert_eq!(plan.body["events"][0]["params"]["debug_mode"], 1);
    }

    #[test]
    fn ip_included_only_when_allowed_and_present() {
        let mut e = event();
        e.context.ip = Some("198.51.100.9".into());
        let plan = collect(&settings(), &e, true);
        assert_eq!(
            plan.body["events"][0]["params"]["ip_override"],
            "198.51.100.9"
        );
        let plan = collect(&settings(), &e, false);
        assert!(plan.body["events"][0]["params"]["ip_override"].is_null());
    }

    #[test]
    fn credentials_are_url_encoded() {
        let mut s = settings();
        s.api_secret = "a/b+c=".into();
        let plan = collect(&s, &event(), false);
        assert!(plan.url.ends_with("api_secret=a%2Fb%2Bc%3D"));
    }

    #[test]
    fn host_is_normalized() {
        let mut s = settings();
        s.host = "www.google-analytics.com/".into();
        let plan = collect(&s, &event(), false);
        assert!(plan.url.starts_with("https://www.google-analytics.com/mp/collect"));
    }
}
