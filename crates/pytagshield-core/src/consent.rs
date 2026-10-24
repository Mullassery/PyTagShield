use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{ConsentConfig, DefaultPolicy};

/// Where a purpose value came from when resolving consent for an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentSource {
    Event,
    Stored,
    Default,
}

/// The effective consent decision for one event, covering every purpose
/// declared in configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConsentSnapshot {
    pub purposes: BTreeMap<String, bool>,
    pub source: ConsentSource,
}

impl ConsentSnapshot {
    /// Whether dispatch to a destination with the given purpose requirement
    /// is permitted. Destinations without a purpose are always permitted
    /// (they are assumed functional).
    pub fn allows(&self, purpose: Option<&str>) -> bool {
        match purpose {
            None => true,
            Some(p) => self.purposes.get(p).copied().unwrap_or(false),
        }
    }

    /// True when at least one purpose is granted. Used to gate enrichment
    /// transformations.
    pub fn any_granted(&self) -> bool {
        self.purposes.values().any(|v| *v)
    }

    /// Only the purposes that are granted (used for outbound disclosure).
    pub fn granted(&self) -> BTreeMap<String, bool> {
        self.purposes
            .iter()
            .filter(|(_, v)| **v)
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }
}

/// Resolve consent for an event, with precedence:
/// event payload > stored consent for the distinct id > configured default.
pub fn resolve(
    event_consent: Option<&BTreeMap<String, bool>>,
    stored: Option<&BTreeMap<String, bool>>,
    cfg: &ConsentConfig,
) -> ConsentSnapshot {
    let default = match cfg.default_policy {
        DefaultPolicy::Deny => false,
        DefaultPolicy::Allow => true,
    };

    let mut purposes = BTreeMap::new();
    let mut source = ConsentSource::Default;
    for purpose in &cfg.purposes {
        let value = if let Some(from_event) = event_consent.and_then(|c| c.get(purpose)) {
            source = ConsentSource::Event;
            *from_event
        } else if let Some(from_store) = stored.and_then(|c| c.get(purpose)) {
            if source != ConsentSource::Event {
                source = ConsentSource::Stored;
            }
            *from_store
        } else {
            default
        };
        purposes.insert(purpose.clone(), value);
    }
    ConsentSnapshot { purposes, source }
}

struct StoredConsent {
    purposes: BTreeMap<String, bool>,
    updated_at: DateTime<Utc>,
}

/// Bounded, TTL'd in-memory consent store keyed by distinct id.
///
/// The store keeps raw identifiers in memory only; identifiers are
/// pseudonymized before any outbound use. Capacity is bounded and the oldest
/// entries are evicted first, so memory use is explicit and predictable.
#[derive(Default)]
pub struct ConsentStore {
    inner: RwLock<HashMap<String, StoredConsent>>,
}

impl ConsentStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace consent. Returns the number of entries evicted to
    /// stay within `max_entries`.
    pub fn set(
        &self,
        distinct_id: &str,
        purposes: BTreeMap<String, bool>,
        now: DateTime<Utc>,
        ttl_secs: u64,
        max_entries: usize,
    ) -> usize {
        let mut map = match self.inner.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut evicted = 0;
        map.retain(|_, v| now.signed_duration_since(v.updated_at).num_seconds() < ttl_secs as i64);
        if map.len() >= max_entries && !map.contains_key(distinct_id) {
            // Evict oldest first until there is room for the new entry.
            while map.len() >= max_entries {
                let oldest = map
                    .iter()
                    .min_by_key(|(_, v)| v.updated_at)
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(k) => {
                        map.remove(&k);
                        evicted += 1;
                    }
                    None => break,
                }
            }
        }
        map.insert(
            distinct_id.to_owned(),
            StoredConsent {
                purposes,
                updated_at: now,
            },
        );
        evicted
    }

    /// Look up consent, ignoring expired entries.
    pub fn get(
        &self,
        distinct_id: &str,
        now: DateTime<Utc>,
        ttl_secs: u64,
    ) -> Option<BTreeMap<String, bool>> {
        let map = match self.inner.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = map.get(distinct_id)?;
        if now.signed_duration_since(entry.updated_at).num_seconds() >= ttl_secs as i64 {
            return None;
        }
        Some(entry.purposes.clone())
    }

    pub fn len(&self) -> usize {
        match self.inner.read() {
            Ok(g) => g.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn cfg() -> ConsentConfig {
        ConsentConfig::default()
    }

    fn map(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn defaults_to_deny() {
        let snap = resolve(None, None, &cfg());
        assert_eq!(snap.source, ConsentSource::Default);
        assert!(!snap.allows(Some("analytics")));
        assert!(!snap.any_granted());
        // Destinations without a purpose requirement are always allowed.
        assert!(snap.allows(None));
    }

    #[test]
    fn event_consent_overrides_stored() {
        let stored = map(&[("analytics", true), ("advertising", true)]);
        let event = map(&[("analytics", false)]);
        let snap = resolve(Some(&event), Some(&stored), &cfg());
        assert_eq!(snap.source, ConsentSource::Event);
        assert!(!snap.allows(Some("analytics")));
        assert!(snap.allows(Some("advertising")));
    }

    #[test]
    fn stored_used_when_event_silent() {
        let stored = map(&[("analytics", true)]);
        let snap = resolve(None, Some(&stored), &cfg());
        assert_eq!(snap.source, ConsentSource::Stored);
        assert!(snap.allows(Some("analytics")));
        assert!(!snap.allows(Some("advertising")));
    }

    #[test]
    fn allow_policy_applies_to_missing_purposes() {
        let mut c = cfg();
        c.default_policy = DefaultPolicy::Allow;
        let snap = resolve(None, None, &c);
        assert!(snap.allows(Some("advertising")));
        assert!(snap.any_granted());
    }

    #[test]
    fn granted_only_contains_true_values() {
        let event = map(&[("analytics", true), ("advertising", false)]);
        let snap = resolve(Some(&event), None, &cfg());
        let granted = snap.granted();
        assert!(granted.contains_key("analytics"));
        assert!(!granted.contains_key("advertising"));
    }

    #[test]
    fn store_roundtrip_and_ttl() {
        let store = ConsentStore::new();
        let now = Utc::now();
        store.set("u1", map(&[("analytics", true)]), now, 3600, 100);
        assert_eq!(
            store.get("u1", now + Duration::seconds(60), 3600),
            Some(map(&[("analytics", true)]))
        );
        assert_eq!(store.get("u1", now + Duration::seconds(3601), 3600), None);
        assert_eq!(store.get("u2", now, 3600), None);
    }

    #[test]
    fn store_evicts_oldest_beyond_capacity() {
        let store = ConsentStore::new();
        let now = Utc::now();
        for i in 0..10 {
            let t = now + Duration::seconds(i);
            store.set(&format!("u{i}"), map(&[("analytics", true)]), t, 3600, 5);
        }
        assert_eq!(store.len(), 5);
        // The five newest entries survive.
        assert!(store.get("u9", now + Duration::seconds(60), 3600).is_some());
        assert!(store.get("u0", now + Duration::seconds(60), 3600).is_none());
    }
}
