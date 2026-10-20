use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::config::MetaSettings;
use crate::destination::{absolute_base, HttpRequestPlan};
use crate::event::Event;

/// Property keys that carry personally identifiable information the
/// Conversions API expects pre-hashed. Mapping them to Meta's field names
/// keeps the payload schema-correct.
const HASHABLE_FIELDS: &[(&str, &str)] = &[
    ("email", "em"),
    ("email_address", "em"),
    ("phone", "ph"),
    ("phone_number", "ph"),
    ("first_name", "fn"),
    ("last_name", "ln"),
    ("city", "ct"),
    ("state", "st"),
    ("zip_code", "zp"),
    ("postal_code", "zp"),
    ("country", "country"),
    ("gender", "ge"),
    ("date_of_birth", "db"),
    ("external_id", "external_id"),
];

/// Build a Meta Conversions API request for one event.
///
/// Payload follows the Graph API `/{pixel_id}/events` schema:
/// `{ data: [ { event_name, event_time, event_id, action_source,
/// user_data, custom_data } ], [test_event_code] }`.
///
/// See: <https://developers.facebook.com/docs/marketing-api/conversions-api>
pub fn events(settings: &MetaSettings, event: &Event, include_ip: bool) -> HttpRequestPlan {
    let mut user_data = Map::new();
    let mut custom_data = Map::new();

    // Move recognised PII properties into hashed user_data fields; everything
    // else is treated as custom_data. Meta requires SHA-256 of normalized
    // lowercase values.
    for (key, value) in &event.properties {
        let lower = key.to_ascii_lowercase();
        match HASHABLE_FIELDS.iter().find(|(src, _)| *src == lower) {
            Some((_, target)) => {
                if let Some(s) = scalar_string(value) {
                    user_data.insert((*target).to_owned(), json!(sha256_hex(&s)));
                }
            }
            None => {
                custom_data.insert(key.clone(), value.clone());
            }
        }
    }

    // Click identifiers and browser cookies go into user_data unhashed, which
    // is what Meta expects for these opaque tokens.
    for (prop, meta_key) in [
        ("fbclid", "fbc"),
        ("_fbp", "fbp"),
        ("fbp", "fbp"),
        ("_fbc", "fbc"),
    ] {
        if let Some(Value::String(v)) = event.properties.get(prop) {
            user_data
                .entry(meta_key.to_owned())
                .or_insert_with(|| json!(v));
        }
    }
    // Preserved attribution block, if present.
    if let Some(Value::Object(attr)) = event.properties.get("$attribution") {
        if let Some(Value::String(v)) = attr.get("fbclid") {
            user_data.entry("fbc".to_owned()).or_insert_with(|| json!(v));
        }
    }

    if include_ip {
        if let Some(ip) = &event.context.ip {
            user_data.insert("client_ip_address".to_owned(), json!(ip));
        }
    }
    if let Some(ua) = &event.context.user_agent {
        user_data.insert("client_user_agent".to_owned(), json!(ua));
    }
    // The pseudonymized distinct id is a stable, non-reversible external id.
    if !event.distinct_id.is_empty() {
        user_data
            .entry("external_id".to_owned())
            .or_insert_with(|| json!(sha256_hex(&event.distinct_id)));
    }

    let mut datum = json!({
        "event_name": event.name,
        "event_time": event.timestamp_millis / 1000,
        "event_id": event.id,
        "action_source": "website",
        "user_data": Value::Object(user_data),
    });
    if !custom_data.is_empty() {
        datum["custom_data"] = Value::Object(custom_data);
    }

    let mut body = json!({ "data": [datum] });
    if let Some(code) = &settings.test_event_code {
        if !code.trim().is_empty() {
            body["test_event_code"] = json!(code);
        }
    }

    let url = format!(
        "{}/{}/{}/events?access_token={}",
        absolute_base(&settings.host),
        settings.api_version.trim_matches('/'),
        settings.pixel_id,
        urlencode(&settings.access_token),
    );

    HttpRequestPlan {
        url,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body,
    }
}

/// SHA-256 hex digest of the trimmed, lowercased input, as Meta requires for
/// hashed user-data fields.
pub fn sha256_hex(value: &str) -> String {
    let normalized = value.trim().to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hex::encode(hasher.finalize())
}

fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
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
    use std::collections::BTreeMap;

    fn event(props: Value) -> Event {
        Event::parse(
            &json!({"event": "Purchase", "distinct_id": "user-1", "properties": props}),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap()
    }

    fn settings() -> MetaSettings {
        MetaSettings {
            pixel_id: "1234567890".into(),
            access_token: "tok".into(),
            host: "https://graph.facebook.com".into(),
            api_version: "v21.0".into(),
            test_event_code: None,
        }
    }

    #[test]
    fn payload_matches_capi_schema() {
        let plan = events(
            &settings(),
            &event(json!({"currency": "USD", "value": 42})),
            false,
        );
        assert_eq!(
            plan.url,
            "https://graph.facebook.com/v21.0/1234567890/events?access_token=tok"
        );
        assert_eq!(plan.body["data"][0]["event_name"], "Purchase");
        assert_eq!(plan.body["data"][0]["action_source"], "website");
        assert_eq!(plan.body["data"][0]["custom_data"]["currency"], "USD");
        assert_eq!(plan.body["data"][0]["custom_data"]["value"], 42);
        assert!(plan.body["test_event_code"].is_null());
    }

    #[test]
    fn pii_is_hashed_into_user_data() {
        let plan = events(&settings(), &event(json!({"email": " A@B.C "})), false);
        let em = plan.body["data"][0]["user_data"]["em"].as_str().unwrap();
        // Trimmed + lowercased before hashing => matches canonical casing.
        assert_eq!(em, sha256_hex("a@b.c"));
        // Raw email must not leak into custom_data.
        assert!(plan.body["data"][0]["custom_data"]["email"].is_null());
    }

    #[test]
    fn click_ids_and_cookies_are_unhashed() {
        let plan = events(
            &settings(),
            &event(json!({"fbclid": "abc", "_fbp": "fb.1.2.3"})),
            false,
        );
        assert_eq!(plan.body["data"][0]["user_data"]["fbc"], "abc");
        assert_eq!(plan.body["data"][0]["user_data"]["fbp"], "fb.1.2.3");
    }

    #[test]
    fn attribution_block_feeds_fbc() {
        let plan = events(
            &settings(),
            &event(json!({"$attribution": {"fbclid": "from-attr"}})),
            false,
        );
        assert_eq!(plan.body["data"][0]["user_data"]["fbc"], "from-attr");
    }

    #[test]
    fn external_id_and_context_populated() {
        let mut e = event(json!({}));
        e.context.user_agent = Some("UA/1".into());
        let plan = events(&settings(), &e, false);
        assert_eq!(plan.body["data"][0]["user_data"]["client_user_agent"], "UA/1");
        assert!(plan.body["data"][0]["user_data"]["external_id"].is_string());
    }

    #[test]
    fn ip_only_when_forwarded() {
        let mut e = event(json!({}));
        e.context.ip = Some("203.0.113.5".into());
        let plan = events(&settings(), &e, true);
        assert_eq!(
            plan.body["data"][0]["user_data"]["client_ip_address"],
            "203.0.113.5"
        );
        let plan = events(&settings(), &e, false);
        assert!(plan.body["data"][0]["user_data"]["client_ip_address"].is_null());
    }

    #[test]
    fn test_event_code_included_when_set() {
        let mut s = settings();
        s.test_event_code = Some("TEST123".into());
        let plan = events(&s, &event(json!({})), false);
        assert_eq!(plan.body["test_event_code"], "TEST123");
    }

    #[test]
    fn event_time_is_seconds() {
        let e = event(json!({}));
        let plan = events(&settings(), &e, false);
        assert_eq!(
            plan.body["data"][0]["event_time"].as_i64().unwrap(),
            e.timestamp_millis / 1000
        );
    }

    #[test]
    fn sha256_normalizes_case_and_whitespace() {
        assert_eq!(sha256_hex("  Foo@Bar.COM "), sha256_hex("foo@bar.com"));
        assert_ne!(sha256_hex("a"), sha256_hex("b"));
    }

    #[test]
    fn _hashing_is_stable() {
        let a: BTreeMap<String, String> =
            [("k".to_string(), sha256_hex("value"))].into_iter().collect();
        assert_eq!(a["k"].len(), 64);
    }
}
