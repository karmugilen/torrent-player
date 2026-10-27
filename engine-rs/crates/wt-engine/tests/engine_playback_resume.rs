//! Integration tests ported from `engine-go/playback_resume_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "needs: playback close/reopen preserving resume across metadata probes"]
#[test]
fn test_playback_close_reopen_preserves_resume_across_metadata_probes() {
    // Port of Go `TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes`.
    for direct in [false, true] {
        let (eng, id, piece_len, data) = support::large_playback_fixture_mode(24, direct);
        for index in [0, 10, 11] {
            support::write_verified_piece(&eng, &id, &data, index);
        }
        let first = eng.server.open_playback(&id, 0).expect("open 1");
        let mut buf = vec![0u8; 64 << 10];
        let n = first.read_at(&mut buf, 10 * piece_len).expect("read");
        assert_eq!(n, buf.len());
        assert_eq!(
            &buf[..],
            &data[(10 * piece_len) as usize..(10 * piece_len) as usize + buf.len()]
        );
        first.close();

        let probe = eng.server.open_playback(&id, 0).expect("open probe");
        let _ = probe.read_at(&mut buf, 0);
        probe.close();

        let reopened = eng.server.open_playback(&id, 0).expect("open reopened");
        let n = reopened
            .read_at(&mut buf, 10 * piece_len)
            .expect("read reopened");
        assert_eq!(n, buf.len());
        reopened.close();
    }
}

#[ignore = "needs: open_playback reusing validated resume window"]
#[test]
fn test_open_playback_reuses_validated_resume_window() {
    // Port of Go `TestOpenPlaybackReusesValidatedResumeWindow`.
    let (data, metadata) = support::recovery_metadata(None);
    let eng = TestEngine::new();
    let id = support::add_recovery_torrent(&eng, &metadata);
    let configure_req = serde_json::json!({
        "id": id,
        "selected": [0],
        "descriptors": [null],
        "storageMode": "stream",
        "streamCacheBytes": 16 << 20,
    });
    let (code, resp) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(
        code,
        200,
        "configure: {code} {}",
        String::from_utf8_lossy(&resp)
    );

    support::write_verified_piece(&eng, &id, &data, 0);
    support::write_verified_piece(&eng, &id, &data, 1);

    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    let mut buf = vec![0u8; 64];
    let n = p.read_at(&mut buf, 0).expect("read_at");
    assert_eq!(n, 64);
    p.close();
}
