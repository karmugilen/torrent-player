//! Map `wt_discovery::TrackerStatusEvent` → `wt_core::StatusEvent`.

use wt_discovery::TrackerStatusEvent;

use crate::StatusEvent;

pub fn map_tracker_status(ev: &TrackerStatusEvent) -> StatusEvent {
    match ev {
        TrackerStatusEvent::Connected { url, info_hash } => StatusEvent::TrackerConnected {
            url: url.clone(),
            info_hash: *info_hash,
        },
        TrackerStatusEvent::Disconnected {
            url,
            info_hash,
            error,
        } => StatusEvent::TrackerDisconnected {
            url: url.clone(),
            info_hash: *info_hash,
            error: error.clone(),
        },
        TrackerStatusEvent::AnnounceSuccessful { url, info_hash } => {
            StatusEvent::TrackerAnnounceSuccessful {
                url: url.clone(),
                info_hash: *info_hash,
            }
        }
        TrackerStatusEvent::AnnounceError {
            url,
            info_hash,
            error,
        } => StatusEvent::TrackerAnnounceError {
            url: url.clone(),
            info_hash: *info_hash,
            error: error.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_types::InfoHash;

    #[test]
    fn maps_all_variants() {
        let ih = InfoHash([1u8; 20]);
        match map_tracker_status(&TrackerStatusEvent::Connected {
            url: "http://t/a".into(),
            info_hash: ih,
        }) {
            StatusEvent::TrackerConnected { .. } => {}
            other => panic!("{other:?}"),
        }
        match map_tracker_status(&TrackerStatusEvent::AnnounceError {
            url: "u".into(),
            info_hash: ih,
            error: "x".into(),
        }) {
            StatusEvent::TrackerAnnounceError { error, .. } => assert_eq!(error, "x"),
            other => panic!("{other:?}"),
        }
    }
}
