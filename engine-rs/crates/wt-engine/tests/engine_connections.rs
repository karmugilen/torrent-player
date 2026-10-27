//! Integration tests ported from `engine-go/connections_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for live peer connections"]
#[test]
fn test_offline_peer_countries() {
    // Port of Go `TestOfflinePeerCountries`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestOfflinePeerCountries");
}

#[ignore = "waiting for live peer connections"]
#[test]
fn test_connections_exclude_candidates_and_unknown_torrent() {
    // Port of Go `TestConnectionsExcludeCandidatesAndUnknownTorrent`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestConnectionsExcludeCandidatesAndUnknownTorrent");
}

#[ignore = "waiting for live peer connections"]
#[test]
fn test_connections_reports_real_tcp_peer_and_clears_on_pause() {
    // Port of Go `TestConnectionsReportsRealTCPPeerAndClearsOnPause`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestConnectionsReportsRealTCPPeerAndClearsOnPause");
}
