//! Go-compatible per-peer upload gate (`PeerConn.uploadAllowed`).
//!
//! `applySeedPolicy` in `engine-go/seeding.go` flips torrent-level
//! Allow/DisallowDataUpload; that flag arrives here as
//! `ChokeConfig.data_upload_disallowed`.

use super::unchoke::{ChokeConfig, PeerChokeInput, TIT_FOR_TAT_HEADROOM};

/// Whether we may upload (and thus unchoke) this peer right now.
///
/// Mirrors anacrolix `uploadAllowed`:
/// 1. `no_upload` / `data_upload_disallowed` → false
/// 2. seeding → true
/// 3. peer has no pieces we want → false
/// 4. uploaded ≥ downloaded + 100 KiB → false (tit-for-tat headroom)
/// 5. else true
pub fn upload_allowed(
    peer: &PeerChokeInput,
    config: &ChokeConfig,
    peer_has_wanted_pieces: bool,
) -> bool {
    if config.no_upload || config.data_upload_disallowed {
        return false;
    }
    if config.seeding {
        return true;
    }
    if !peer_has_wanted_pieces {
        return false;
    }
    let headroom = if config.tit_for_tat_headroom >= 0 {
        config.tit_for_tat_headroom
    } else {
        TIT_FOR_TAT_HEADROOM
    };
    if peer.bytes_uploaded >= peer.bytes_downloaded.saturating_add(headroom) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::PeerChokeInput;
    use super::*;

    fn peer(up: i64, down: i64) -> PeerChokeInput {
        PeerChokeInput {
            id: 1,
            interested: true,
            currently_choking: true,
            download_rate: 0.0,
            upload_rate: 0.0,
            bytes_downloaded: down,
            bytes_uploaded: up,
            is_snubbed: false,
        }
    }

    #[test]
    fn disallowed_when_seed_policy_blocks() {
        let mut cfg = ChokeConfig::default();
        cfg.data_upload_disallowed = true;
        assert!(!upload_allowed(&peer(0, 0), &cfg, true));
    }

    #[test]
    fn seeding_always_allows() {
        let mut cfg = ChokeConfig::default();
        cfg.seeding = true;
        assert!(upload_allowed(&peer(1_000_000, 0), &cfg, false));
    }

    #[test]
    fn tit_for_tat_headroom() {
        let cfg = ChokeConfig::default();
        // Go: up >= down + 100KiB → disallow. Equality counts as disallow.
        assert!(!upload_allowed(&peer(100 << 10, 0), &cfg, true));
        assert!(upload_allowed(&peer((100 << 10) - 1, 0), &cfg, true));
        assert!(!upload_allowed(&peer(0, 0), &cfg, false)); // no wanted pieces
    }
}
