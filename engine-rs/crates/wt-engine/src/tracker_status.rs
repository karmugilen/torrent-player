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
