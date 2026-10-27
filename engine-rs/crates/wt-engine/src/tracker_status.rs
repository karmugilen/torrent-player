//! Live tracker status tracking and views.
//!
//! Port of `engine-go/tracker_status.go`.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

use crate::types::TrackerView;

pub const TRACKER_STATUS_WAITING: &str = "waiting";
pub const TRACKER_STATUS_WORKING: &str = "working";
pub const TRACKER_STATUS_ERROR: &str = "error";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerLiveStatus {
    pub status: String,
    pub message: String,
}

#[derive(Default)]
pub struct TrackerStatusManager {
    inner: RwLock<HashMap<String, HashMap<String, TrackerLiveStatus>>>,
    on_change: RwLock<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
}

impl TrackerStatusManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_status(&self, info_hash: &str, url: &str, status: &str, message: &str) -> bool {
        let info_hash = info_hash.trim().to_ascii_lowercase();
        let url = url.trim();
        if info_hash.is_empty() || url.is_empty() {
            return false;
        }

        let next = TrackerLiveStatus {
            status: status.to_string(),
            message: message.to_string(),
        };

        let mut lock = self.inner.write();
        let by_url = lock.entry(info_hash).or_default();
        if let Some(existing) = by_url.get(url) {
            if existing == &next {
                return false;
            }
        }
        by_url.insert(url.to_string(), next);
        drop(lock);
        if let Some(callback) = self.on_change.read().clone() {
            callback();
        }
        true
    }

    pub fn clear_status(&self, info_hash: &str) {
        let info_hash = info_hash.trim().to_ascii_lowercase();
        if !info_hash.is_empty() {
            self.inner.write().remove(&info_hash);
        }
    }

    pub fn snapshot(&self, info_hash: &str) -> HashMap<String, TrackerLiveStatus> {
        let info_hash = info_hash.trim().to_ascii_lowercase();
        let lock = self.inner.read();
        lock.get(&info_hash).cloned().unwrap_or_default()
    }
}

pub fn format_peer_count(n: usize) -> String {
    if n == 1 {
        "1 peer".to_string()
    } else {
        format!("{n} peers")
    }
}

pub fn build_tracker_views(
    original_trackers: &[Vec<String>],
    announce_list: &[Vec<String>],
    live: &HashMap<String, TrackerLiveStatus>,
) -> Vec<TrackerView> {
    let mut original = HashSet::new();
    for tier in original_trackers {
        for u in tier {
            let u = u.trim();
            if !u.is_empty() {
                original.insert(u.to_string());
            }
        }
    }

    let tiers = if !announce_list.is_empty() {
        announce_list
    } else {
        original_trackers
    };

    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for (tier_idx, tier) in tiers.iter().enumerate() {
        for raw in tier {
            let u = raw.trim();
            if u.is_empty() || !seen.insert(u.to_string()) {
                continue;
            }
            let is_original = original.contains(u);
            let mut view = TrackerView {
                url: u.to_string(),
                tier: tier_idx,
                status: TRACKER_STATUS_WAITING.to_string(),
                message: None,
                original: is_original,
            };

            if let Some(st) = live.get(u) {
                view.status = st.status.clone();
                if !st.message.is_empty() {
                    view.message = Some(st.message.clone());
                }
            }

            out.push(view);
        }
    }

    out
}

impl TrackerStatusManager {
    /// Structured announce diagnostics; no identity-free fallback is created.
    pub fn announce(&self, info_hash: &str, url: &str, result: Result<usize, &str>) -> bool {
        match result {
            Ok(peers) => self.set_status(
                info_hash,
                url,
                TRACKER_STATUS_WORKING,
                &format_peer_count(peers),
            ),
            Err(error) => self.set_status(info_hash, url, TRACKER_STATUS_ERROR, error),
        }
    }

    pub fn connection_status(&self, info_hash: &str, url: &str, error: Option<&str>) -> bool {
        self.set_status(
            info_hash,
            url,
            if error.is_some() {
                TRACKER_STATUS_ERROR
            } else {
                TRACKER_STATUS_WORKING
            },
            error.unwrap_or(""),
        )
    }

    pub fn on_change(&self, callback: std::sync::Arc<dyn Fn() + Send + Sync>) {
        *self.on_change.write() = Some(callback);
    }
}

/// Retains tracker identity from bound tracing spans while leaving downstream
/// layers and their filtering unchanged. Install with `registry().with(layer)`.
pub struct TrackerStatusLayer {
    manager: std::sync::Arc<TrackerStatusManager>,
}

impl TrackerStatusLayer {
    pub fn new(manager: std::sync::Arc<TrackerStatusManager>) -> Self {
        Self { manager }
    }

    fn note_announce(&self, fields: &HashMap<String, String>) {
        let Some(url) = fields.get("url") else {
            return;
        };
        let Some(hash) = fields
            .get("info_hash")
            .or_else(|| fields.get("short infohash"))
        else {
            return;
        };
        if let Some(error) = fields
            .get("err")
            .or_else(|| fields.get("error"))
            .filter(|e| !matches!(e.as_str(), "" | "None" | "<nil>"))
        {
            self.manager
                .set_status(hash, url, TRACKER_STATUS_ERROR, error);
        } else {
            let message = fields
                .get("peers")
                .or_else(|| fields.get("peer_count"))
                .and_then(|n| n.parse().ok())
                .map(format_peer_count)
                .unwrap_or_default();
            self.manager
                .set_status(hash, url, TRACKER_STATUS_WORKING, &message);
        }
    }
}
#[derive(Default)]
struct TraceFields(HashMap<String, String>);

impl tracing::field::Visit for TraceFields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

impl<S> tracing_subscriber::Layer<S> for TrackerStatusLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            let mut fields = TraceFields::default();
            attrs.record(&mut fields);
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            if let Some(fields) = span.extensions_mut().get_mut::<TraceFields>() {
                values.record(fields);
            }
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut fields = HashMap::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(bound) = span.extensions().get::<TraceFields>() {
                    for (key, value) in &bound.0 {
                        fields.insert(
                            key.rsplit('.').next().unwrap_or(key).to_string(),
                            value.clone(),
                        );
                    }
                }
            }
        }
        let mut local = TraceFields::default();
        event.record(&mut local);
        for (key, value) in local.0 {
            fields.insert(key.rsplit('.').next().unwrap_or(&key).to_string(), value);
        }
        if fields
            .get("message")
            .is_some_and(|message| message == "announced")
        {
            self.note_announce(&fields);
        }
    }
}

#[cfg(test)]
mod tracing_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tracing_subscriber::prelude::*;

    struct Downstream(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Downstream {
        fn on_event(&self, _: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn bound_span_identity_and_errors_are_captured_and_forwarded() {
        let manager = Arc::new(TrackerStatusManager::new());
        let seen = Arc::new(AtomicUsize::new(0));
        let sub = tracing_subscriber::registry()
            .with(TrackerStatusLayer::new(manager.clone()))
            .with(Downstream(seen.clone()));
        tracing::subscriber::with_default(sub, || {
            let outer = tracing::debug_span!(
                "torrent",
                info_hash = "ABCDEF",
                url = "udp://one.example.org:1337/announce"
            );
            let _guard = outer.enter();
            tracing::debug!(peers = 1u64, "announced");
            let status = manager.snapshot("abcdef");
            assert_eq!(status.values().next().unwrap().message, "1 peer");
            {
                let inner = tracing::debug_span!(
                    "nested",
                    info_hash = "another",
                    url = "https://two.example.org"
                );
                let _guard = inner.enter();
                tracing::debug!(err = "tracker unavailable", "announced");
            }
            tracing::debug!(peers = 4u64, "announced");
            tracing::debug!("unrelated diagnostics");
        });
        assert_eq!(
            manager.snapshot("abcdef")["udp://one.example.org:1337/announce"].message,
            "4 peers"
        );
        assert_eq!(
            manager.snapshot("another")["https://two.example.org"].status,
            TRACKER_STATUS_ERROR
        );
        assert_eq!(seen.load(Ordering::SeqCst), 4);
        assert!(manager.snapshot("").is_empty());
    }
}
