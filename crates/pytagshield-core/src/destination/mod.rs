//! Outbound request construction for destination types.

pub mod ga4;
pub mod meta;
pub mod posthog;
pub mod sgtm;
pub mod webhook;

use serde::Serialize;
use serde_json::Value;

use crate::config::{DestinationConfig, DestinationType};
use crate::consent::ConsentSnapshot;
use crate::event::Event;

/// A fully prepared outbound HTTP request.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HttpRequestPlan {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

/// What the delivery worker should do with one event for one destination.
#[derive(Clone, Debug, PartialEq)]
pub enum OutboundPlan {
    Http(HttpRequestPlan),
    /// Local, side-effect-free destination used for routing diagnostics.
    Log {
        detail: String,
    },
}

/// Build the outbound plan for `event` at `dest`.
///
/// Errors are explicit: a misconfigured destination fails the delivery and
/// the failure is recorded, never silently dropped.
pub fn build(
    dest: &DestinationConfig,
    event: &Event,
    consent: &ConsentSnapshot,
) -> Result<OutboundPlan, String> {
    match dest.kind {
        DestinationType::Posthog => {
            let settings = dest
                .posthog
                .as_ref()
                .ok_or_else(|| format!("destination '{}': missing posthog settings", dest.name))?;
            let plan = posthog::capture(settings, event, dest.forwards_ip());
            Ok(OutboundPlan::Http(plan))
        }
        DestinationType::Ga4 => {
            let settings = dest
                .ga4
                .as_ref()
                .ok_or_else(|| format!("destination '{}': missing ga4 settings", dest.name))?;
            let plan = ga4::collect(settings, event, dest.forwards_ip());
            Ok(OutboundPlan::Http(plan))
        }
        DestinationType::Meta => {
            let settings = dest
                .meta
                .as_ref()
                .ok_or_else(|| format!("destination '{}': missing meta settings", dest.name))?;
            let plan = meta::events(settings, event, dest.forwards_ip());
            Ok(OutboundPlan::Http(plan))
        }
        DestinationType::Sgtm => {
            let settings = dest
                .sgtm
                .as_ref()
                .ok_or_else(|| format!("destination '{}': missing sgtm settings", dest.name))?;
            let plan = sgtm::collect(settings, event, dest.forwards_ip());
            Ok(OutboundPlan::Http(plan))
        }
        DestinationType::Webhook => {
            let settings = dest
                .webhook
                .as_ref()
                .ok_or_else(|| format!("destination '{}': missing webhook settings", dest.name))?;
            Ok(OutboundPlan::Http(webhook::request(
                settings, event, consent,
            )))
        }
        DestinationType::Log => Ok(OutboundPlan::Log {
            detail: format!(
                "event '{}' ({}) routed to log destination '{}'",
                event.name, event.id, dest.name
            ),
        }),
    }
}

/// Normalize a configured host into an absolute base URL.
pub(crate) fn absolute_base(host: &str) -> String {
    let trimmed = host.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IngestConfig, PosthogSettings, WebhookSettings};
    use chrono::Utc;
    use serde_json::json;

    fn event() -> Event {
        Event::parse(
            &json!({"event":"page_view","distinct_id":"u1","properties":{"url":"/x"}}),
            &IngestConfig::default(),
            Utc::now(),
        )
        .unwrap()
    }

    fn any_consent() -> crate::consent::ConsentSnapshot {
        use crate::config::ConsentConfig;
        use std::collections::BTreeMap;
        let cfg = ConsentConfig::default();
        let purposes: BTreeMap<String, bool> =
            cfg.purposes.iter().map(|p| (p.clone(), true)).collect();
        crate::consent::resolve(Some(&purposes), None, &cfg)
    }

    #[test]
    fn posthog_plan_is_capture_request() {
        let dest = DestinationConfig {
            name: "ph".into(),
            kind: DestinationType::Posthog,
            purpose: None,
            enabled: true,
            forward_ip: None,
            queue_capacity: None,
            posthog: Some(PosthogSettings {
                api_key: "phc_key".into(),
                host: "us.i.posthog.com".into(),
            }),
            ga4: None,
            meta: None,
            sgtm: None,
            webhook: None,
        };
        let plan = build(&dest, &event(), &any_consent()).unwrap();
        match plan {
            OutboundPlan::Http(req) => {
                assert_eq!(req.url, "https://us.i.posthog.com/capture/");
                assert!(req.body["api_key"].as_str().unwrap().ends_with("phc_key"));
                assert_eq!(req.body["event"], "page_view");
            }
            other => panic!("expected http plan, got {other:?}"),
        }
    }

    #[test]
    fn posthog_plan_propagates_ip_when_allowed() {
        let mut e = event();
        e.context.ip = Some("198.51.100.7".into());
        let dest = DestinationConfig {
            name: "ph".into(),
            kind: DestinationType::Posthog,
            purpose: None,
            enabled: true,
            forward_ip: Some(true),
            queue_capacity: None,
            posthog: Some(PosthogSettings {
                api_key: "k".into(),
                host: "https://eu.i.posthog.com".into(),
            }),
            ga4: None,
            meta: None,
            sgtm: None,
            webhook: None,
        };
        let OutboundPlan::Http(req) = build(&dest, &e, &any_consent()).unwrap() else {
            panic!("expected http");
        };
        assert_eq!(req.url, "https://eu.i.posthog.com/capture/");
        assert_eq!(req.body["properties"]["$ip"], "198.51.100.7");
    }

    #[test]
    fn webhook_plan_includes_envelope() {
        let dest = DestinationConfig {
            name: "wh".into(),
            kind: DestinationType::Webhook,
            purpose: None,
            enabled: true,
            forward_ip: None,
            queue_capacity: None,
            posthog: None,
            ga4: None,
            meta: None,
            sgtm: None,
            webhook: Some(WebhookSettings {
                url: "https://hooks.example.com/ingest".into(),
                headers: [("X-Token".to_string(), "t".to_string())].into(),
            }),
        };
        let OutboundPlan::Http(req) = build(&dest, &event(), &any_consent()).unwrap() else {
            panic!("expected http");
        };
        assert_eq!(req.url, "https://hooks.example.com/ingest");
        assert!(req
            .headers
            .contains(&("X-Token".to_string(), "t".to_string())));
        assert_eq!(req.body["event"], "page_view");
        assert!(req.body["distinct_id"].is_string());
    }

    #[test]
    fn log_plan_never_leaves_process() {
        let dest = DestinationConfig {
            name: "debug".into(),
            kind: DestinationType::Log,
            purpose: None,
            enabled: true,
            forward_ip: None,
            queue_capacity: None,
            posthog: None,
            ga4: None,
            meta: None,
            sgtm: None,
            webhook: None,
        };
        let plan = build(&dest, &event(), &any_consent()).unwrap();
        assert!(matches!(plan, OutboundPlan::Log { .. }));
    }

    #[test]
    fn absolute_base_normalization() {
        assert_eq!(
            absolute_base("us.i.posthog.com"),
            "https://us.i.posthog.com"
        );
        assert_eq!(absolute_base("https://example.com/"), "https://example.com");
        assert_eq!(
            absolute_base("http://localhost:8000"),
            "http://localhost:8000"
        );
    }
}
