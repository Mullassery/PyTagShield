use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::PipelineError;

/// Top-level configuration for a PyTagShield instance.
///
/// The configuration file is strict (`deny_unknown_fields`): typos fail
/// startup instead of being silently ignored.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub ingest: IngestConfig,
    #[serde(default)]
    pub consent: ConsentConfig,
    #[serde(default)]
    pub minimization: MinimizationConfig,
    #[serde(default)]
    pub transforms: Vec<TransformRule>,
    #[serde(default)]
    pub destinations: Vec<DestinationConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
    #[serde(default)]
    pub delivery: DeliveryConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Socket address to bind, e.g. `127.0.0.1:8080`.
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Maximum size of a request body in bytes.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// Maximum number of requests processed concurrently.
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: usize,
}

fn default_listen() -> String {
    "127.0.0.1:8080".to_owned()
}
fn default_max_body_bytes() -> usize {
    1_048_576
}
fn default_max_concurrent_requests() -> usize {
    64
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            max_body_bytes: default_max_body_bytes(),
            max_concurrent_requests: default_max_concurrent_requests(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestConfig {
    /// Optional bearer token required on ingest and consent-write endpoints.
    /// When `None`, ingest is unauthenticated (fine for localhost only).
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default = "default_true")]
    pub require_distinct_id: bool,
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    /// Acceptable clock skew for explicit timestamps, in seconds, both past
    /// and future.
    #[serde(default = "default_timestamp_skew")]
    pub max_timestamp_skew_secs: i64,
    #[serde(default = "default_max_event_name_len")]
    pub max_event_name_len: usize,
    #[serde(default = "default_max_distinct_id_len")]
    pub max_distinct_id_len: usize,
    #[serde(default = "default_max_property_key_len")]
    pub max_property_key_len: usize,
    /// Hard rejection limit for the number of properties on one event.
    #[serde(default = "default_max_properties_per_event")]
    pub max_properties_per_event: usize,
}

fn default_true() -> bool {
    true
}
fn default_max_batch_size() -> usize {
    100
}
fn default_timestamp_skew() -> i64 {
    12 * 60 * 60
}
fn default_max_event_name_len() -> usize {
    128
}
fn default_max_distinct_id_len() -> usize {
    512
}
fn default_max_property_key_len() -> usize {
    128
}
fn default_max_properties_per_event() -> usize {
    500
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            token: None,
            require_distinct_id: true,
            max_batch_size: default_max_batch_size(),
            max_timestamp_skew_secs: default_timestamp_skew(),
            max_event_name_len: default_max_event_name_len(),
            max_distinct_id_len: default_max_distinct_id_len(),
            max_property_key_len: default_max_property_key_len(),
            max_properties_per_event: default_max_properties_per_event(),
        }
    }
}

/// Policy applied when neither the event nor the consent store carries a
/// value for a purpose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultPolicy {
    #[default]
    Deny,
    Allow,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentConfig {
    /// Purposes understood by this instance.
    #[serde(default = "default_purposes")]
    pub purposes: Vec<String>,
    /// Fail-closed (`deny`) unless explicitly granted.
    #[serde(default)]
    pub default_policy: DefaultPolicy,
    #[serde(default = "default_consent_ttl")]
    pub store_ttl_secs: u64,
    #[serde(default = "default_consent_store_max")]
    pub store_max_entries: usize,
}

fn default_purposes() -> Vec<String> {
    ["analytics", "advertising", "personalization", "functional"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
fn default_consent_ttl() -> u64 {
    7 * 24 * 60 * 60
}
fn default_consent_store_max() -> usize {
    10_000
}

impl Default for ConsentConfig {
    fn default() -> Self {
        Self {
            purposes: default_purposes(),
            default_policy: DefaultPolicy::Deny,
            store_ttl_secs: default_consent_ttl(),
            store_max_entries: default_consent_store_max(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinimizationConfig {
    /// Hash distinct identifiers before they leave the process.
    #[serde(default = "default_true")]
    pub pseudonymize_ids: bool,
    /// Secret salt for pseudonymization. Required when pseudonymization is on.
    #[serde(default)]
    pub pseudonymize_salt: String,
    /// Properties removed unconditionally (case-insensitive).
    #[serde(default = "default_drop_properties")]
    pub drop_properties: Vec<String>,
    /// When non-empty, only these properties survive (case-insensitive).
    #[serde(default)]
    pub allowlist_properties: Vec<String>,
    /// Maximum properties kept; excess keys are dropped deterministically
    /// (alphabetical order).
    #[serde(default = "default_max_properties")]
    pub max_properties: usize,
    /// Strings longer than this are truncated.
    #[serde(default = "default_max_value_string_len")]
    pub max_value_string_len: usize,
    /// Values nested deeper than this are dropped.
    #[serde(default = "default_max_nesting_depth")]
    pub max_nesting_depth: usize,
    /// Keep the client IP on the event context for outbound delivery.
    #[serde(default)]
    pub forward_ip: bool,
}

fn default_drop_properties() -> Vec<String> {
    [
        "email",
        "email_address",
        "phone",
        "phone_number",
        "mobile",
        "full_name",
        "first_name",
        "last_name",
        "address",
        "street",
        "postal_code",
        "zip_code",
        "ssn",
        "ip",
        "user_ip",
        "client_ip",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn default_max_properties() -> usize {
    100
}
fn default_max_value_string_len() -> usize {
    4096
}
fn default_max_nesting_depth() -> usize {
    6
}

impl Default for MinimizationConfig {
    fn default() -> Self {
        Self {
            pseudonymize_ids: true,
            pseudonymize_salt: String::new(),
            drop_properties: default_drop_properties(),
            allowlist_properties: Vec::new(),
            max_properties: default_max_properties(),
            max_value_string_len: default_max_value_string_len(),
            max_nesting_depth: default_max_nesting_depth(),
            forward_ip: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryConfig {
    #[serde(default = "default_delivery_log_capacity")]
    pub log_capacity: usize,
    #[serde(default = "default_http_timeout")]
    pub http_timeout_secs: u64,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_initial_backoff")]
    pub initial_backoff_ms: u64,
    #[serde(default = "default_max_backoff")]
    pub max_backoff_ms: u64,
    /// Per-destination queue capacity unless the destination overrides it.
    #[serde(default = "default_queue_capacity")]
    pub default_queue_capacity: usize,
}

fn default_delivery_log_capacity() -> usize {
    1_000
}
fn default_http_timeout() -> u64 {
    5
}
fn default_max_attempts() -> u32 {
    3
}
fn default_initial_backoff() -> u64 {
    100
}
fn default_max_backoff() -> u64 {
    5_000
}
fn default_queue_capacity() -> usize {
    1_024
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            log_capacity: default_delivery_log_capacity(),
            http_timeout_secs: default_http_timeout(),
            max_attempts: default_max_attempts(),
            initial_backoff_ms: default_initial_backoff(),
            max_backoff_ms: default_max_backoff(),
            default_queue_capacity: default_queue_capacity(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationType {
    Posthog,
    Webhook,
    Log,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PosthogSettings {
    pub api_key: String,
    /// Capture host, e.g. `https://us.i.posthog.com` or `us.i.posthog.com`.
    #[serde(default = "default_posthog_host")]
    pub host: String,
}

fn default_posthog_host() -> String {
    "https://us.i.posthog.com".to_owned()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookSettings {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: DestinationType,
    /// Consent purpose required before events may be delivered here.
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Restrict IP forwarding for this destination. `false` guarantees the
    /// client IP is never sent here. Widening beyond the global
    /// `minimization.forward_ip` setting is not possible: the pipeline clears
    /// the IP before routing when the global switch is off.
    #[serde(default)]
    pub forward_ip: Option<bool>,
    /// Override the default queue capacity for this destination.
    #[serde(default)]
    pub queue_capacity: Option<usize>,
    #[serde(default)]
    pub posthog: Option<PosthogSettings>,
    #[serde(default)]
    pub webhook: Option<WebhookSettings>,
}

impl DestinationConfig {
    /// Queue capacity for this destination after applying defaults.
    pub fn effective_queue_capacity(&self, delivery: &DeliveryConfig) -> usize {
        self.queue_capacity.unwrap_or(delivery.default_queue_capacity)
    }

    /// Whether the client IP may be forwarded to this destination, given the
    /// pipeline has already cleared it when the global switch is off.
    pub fn forwards_ip(&self) -> bool {
        self.forward_ip.unwrap_or(true)
    }
}

/// Glob-lite pattern used for event name matching.
/// Supports `*` (any run of characters), `?` (one character) and exact text.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MatchRule {
    /// Event name patterns. Default: `["*"]` (match everything).
    #[serde(default = "default_match_events")]
    pub events: Vec<String>,
    /// Property conditions: every entry must match; a property matches when
    /// its value equals one of the listed values. A missing property matches
    /// only when `null` is listed.
    #[serde(default)]
    pub properties: BTreeMap<String, Vec<serde_json::Value>>,
}

fn default_match_events() -> Vec<String> {
    vec!["*".to_owned()]
}

impl Default for MatchRule {
    fn default() -> Self {
        Self {
            events: default_match_events(),
            properties: BTreeMap::new(),
        }
    }
}

impl MatchRule {
    pub fn is_match_all(&self) -> bool {
        self.properties.is_empty() && self.events.iter().any(|e| e == "*")
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    pub name: String,
    pub destinations: Vec<String>,
    #[serde(default, rename = "match")]
    pub rule: MatchRule,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A deterministic transformation step.
///
/// `rename`, `drop` and `map` are reductive/deterministic and always run.
/// `set` is enrichment and only runs when the event has at least one
/// granted purpose.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Rename { from: String, to: String },
    Drop { keys: Vec<String> },
    Set { key: String, value: serde_json::Value },
    Map {
        key: String,
        from: BTreeMap<String, serde_json::Value>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformRule {
    /// When omitted the rule applies to every event.
    #[serde(default)]
    pub when: Option<MatchRule>,
    pub actions: Vec<Action>,
}

impl Config {
    /// Load and validate a configuration file (YAML).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PipelineError> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .map_err(|e| PipelineError::Config(format!("cannot read {}: {e}", path.display())))?;
        Self::from_yaml(&raw)
    }

    /// Parse and validate configuration from YAML text.
    pub fn from_yaml(raw: &str) -> Result<Self, PipelineError> {
        let config: Config = serde_yaml::from_str(raw)
            .map_err(|e| PipelineError::Config(format!("invalid configuration: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate cross-field invariants. Called on load so a broken
    /// configuration never starts a server.
    pub fn validate(&self) -> Result<(), PipelineError> {
        let err = |msg: String| Err(PipelineError::Config(msg));

        if self.server.max_body_bytes < 1024 {
            return err("server.max_body_bytes must be at least 1024".into());
        }
        if self.server.max_concurrent_requests == 0 {
            return err("server.max_concurrent_requests must be > 0".into());
        }
        if self.ingest.max_batch_size == 0 || self.ingest.max_batch_size > 10_000 {
            return err("ingest.max_batch_size must be in 1..=10000".into());
        }
        if self.ingest.max_properties_per_event == 0 {
            return err("ingest.max_properties_per_event must be > 0".into());
        }

        if self.consent.purposes.is_empty() {
            return err("consent.purposes must not be empty".into());
        }
        let mut seen_purposes = std::collections::HashSet::new();
        for p in &self.consent.purposes {
            if p.trim().is_empty() {
                return err("consent.purposes contains an empty name".into());
            }
            if !seen_purposes.insert(p) {
                return err(format!("consent.purposes contains duplicate '{p}'"));
            }
        }
        if self.consent.store_max_entries == 0 {
            return err("consent.store_max_entries must be > 0".into());
        }

        if self.minimization.pseudonymize_ids && self.minimization.pseudonymize_salt.is_empty() {
            return err(
                "minimization.pseudonymize_salt is required when pseudonymize_ids is true".into(),
            );
        }
        if self.minimization.max_properties == 0 {
            return err("minimization.max_properties must be > 0".into());
        }
        if self.minimization.max_nesting_depth == 0 {
            return err("minimization.max_nesting_depth must be > 0".into());
        }

        if self.delivery.max_attempts == 0 {
            return err("delivery.max_attempts must be > 0".into());
        }
        if self.delivery.initial_backoff_ms > self.delivery.max_backoff_ms {
            return err("delivery.initial_backoff_ms must be <= delivery.max_backoff_ms".into());
        }
        if self.delivery.default_queue_capacity == 0 {
            return err("delivery.default_queue_capacity must be > 0".into());
        }
        if self.delivery.log_capacity == 0 {
            return err("delivery.log_capacity must be > 0".into());
        }

        let mut seen_dest = std::collections::HashSet::new();
        for d in &self.destinations {
            if d.name.trim().is_empty() {
                return err("destinations contain an empty name".into());
            }
            if !seen_dest.insert(&d.name) {
                return err(format!("duplicate destination name '{}'", d.name));
            }
            if let Some(p) = &d.purpose {
                if !self.consent.purposes.contains(p) {
                    return err(format!(
                        "destination '{}': purpose '{p}' is not declared in consent.purposes",
                        d.name
                    ));
                }
            }
            if let Some(cap) = d.queue_capacity {
                if cap == 0 {
                    return err(format!("destination '{}': queue_capacity must be > 0", d.name));
                }
            }
            match d.kind {
                DestinationType::Posthog => {
                    let Some(ph) = &d.posthog else {
                        return err(format!(
                            "destination '{}': type 'posthog' requires a 'posthog' block",
                            d.name
                        ));
                    };
                    if ph.api_key.trim().is_empty() {
                        return err(format!(
                            "destination '{}': posthog.api_key must not be empty",
                            d.name
                        ));
                    }
                    if ph.host.trim().is_empty() {
                        return err(format!(
                            "destination '{}': posthog.host must not be empty",
                            d.name
                        ));
                    }
                    if d.webhook.is_some() {
                        return err(format!(
                            "destination '{}': 'webhook' block is not allowed for type 'posthog'",
                            d.name
                        ));
                    }
                }
                DestinationType::Webhook => {
                    let Some(wh) = &d.webhook else {
                        return err(format!(
                            "destination '{}': type 'webhook' requires a 'webhook' block",
                            d.name
                        ));
                    };
                    if !(wh.url.starts_with("http://") || wh.url.starts_with("https://")) {
                        return err(format!(
                            "destination '{}': webhook.url must start with http:// or https://",
                            d.name
                        ));
                    }
                    if d.posthog.is_some() {
                        return err(format!(
                            "destination '{}': 'posthog' block is not allowed for type 'webhook'",
                            d.name
                        ));
                    }
                }
                DestinationType::Log => {
                    if d.posthog.is_some() || d.webhook.is_some() {
                        return err(format!(
                            "destination '{}': type 'log' takes no settings blocks",
                            d.name
                        ));
                    }
                }
            }
        }

        for r in &self.routes {
            if r.name.trim().is_empty() {
                return err("routes contain an empty name".into());
            }
            if r.destinations.is_empty() {
                return err(format!("route '{}': destinations must not be empty", r.name));
            }
            for dest in &r.destinations {
                if !seen_dest.contains(dest) {
                    return err(format!(
                        "route '{}': unknown destination '{dest}'",
                        r.name
                    ));
                }
            }
            if r.rule.events.is_empty() {
                return err(format!("route '{}': match.events must not be empty", r.name));
            }
        }

        for (i, t) in self.transforms.iter().enumerate() {
            if t.actions.is_empty() {
                return err(format!("transforms[{i}]: actions must not be empty"));
            }
            for (j, a) in t.actions.iter().enumerate() {
                match a {
                    Action::Rename { from, to } => {
                        if from.trim().is_empty() || to.trim().is_empty() {
                            return err(format!("transforms[{i}].actions[{j}]: empty rename"));
                        }
                    }
                    Action::Drop { keys } => {
                        if keys.is_empty() {
                            return err(format!("transforms[{i}].actions[{j}]: empty drop"));
                        }
                    }
                    Action::Set { key, .. } => {
                        if key.trim().is_empty() {
                            return err(format!("transforms[{i}].actions[{j}]: empty set key"));
                        }
                    }
                    Action::Map { key, from } => {
                        if key.trim().is_empty() || from.is_empty() {
                            return err(format!(
                                "transforms[{i}].actions[{j}]: map needs a key and a non-empty from"
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> String {
        r#"
minimization:
  pseudonymize_salt: "test-salt"
destinations:
  - name: log
    type: log
routes:
  - name: all
    destinations: [log]
"#
        .to_owned()
    }

    #[test]
    fn parses_minimal_config_with_defaults() {
        let cfg = Config::from_yaml(&minimal()).expect("valid");
        assert_eq!(cfg.server.listen, "127.0.0.1:8080");
        assert_eq!(cfg.destinations.len(), 1);
        assert_eq!(cfg.consent.default_policy, DefaultPolicy::Deny);
        assert!(cfg.minimization.pseudonymize_ids);
        assert_eq!(cfg.minimization.pseudonymize_salt, "test-salt");
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "destinatins: []".to_owned();
        assert!(Config::from_yaml(&yaml).is_err());
    }

    #[test]
    fn rejects_pseudonymization_without_salt() {
        let yaml = "minimization:\n  pseudonymize_ids: true\ndestinations:\n  - name: log\n    type: log\n";
        let err = Config::from_yaml(yaml).expect_err("must fail");
        assert!(err.to_string().contains("pseudonymize_salt"));
    }

    #[test]
    fn rejects_unknown_route_destination() {
        let yaml = r#"
minimization:
  pseudonymize_salt: "s"
destinations:
  - name: log
    type: log
routes:
  - name: all
    destinations: [nope]
"#;
        let err = Config::from_yaml(yaml).expect_err("must fail");
        assert!(err.to_string().contains("unknown destination 'nope'"));
    }

    #[test]
    fn rejects_wrong_type_blocks() {
        let yaml = r#"
minimization:
  pseudonymize_salt: "s"
destinations:
  - name: ph
    type: posthog
    webhook:
      url: "https://example.com"
"#;
        let err = Config::from_yaml(yaml).expect_err("must fail");
        assert!(err.to_string().contains("requires a 'posthog' block"));
    }

    #[test]
    fn rejects_unknown_purpose_on_destination() {
        let yaml = r#"
minimization:
  pseudonymize_salt: "s"
consent:
  purposes: [analytics]
destinations:
  - name: ph
    type: posthog
    purpose: advertising
    posthog:
      api_key: k
"#;
        let err = Config::from_yaml(yaml).expect_err("must fail");
        assert!(err.to_string().contains("not declared in consent.purposes"));
    }

    #[test]
    fn rejects_duplicate_destination_names() {
        let yaml = r#"
minimization:
  pseudonymize_salt: "s"
destinations:
  - name: log
    type: log
  - name: log
    type: log
"#;
        let err = Config::from_yaml(yaml).expect_err("must fail");
        assert!(err.to_string().contains("duplicate destination"));
    }

    #[test]
    fn loads_example_documented_config() {
        // Keep docs/config/example.yaml honest.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/example.yaml"
        );
        if Path::new(path).exists() {
            Config::load(path).expect("example config must load");
        }
    }
}
