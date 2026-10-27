//! Integration tests ported from `engine-go/live_preview_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;

#[ignore = "needs: preview-state tracking live playback position"]
#[test]
fn test_live_preview_state_follows_verified_playback_position() {
    // Port of Go `TestLivePreviewStateFollowsVerifiedPlaybackPosition`.
    let (eng, id, piece_len, data) = support::large_playback_fixture_mode(24, true);
    support::write_verified_piece(&eng, &id, &data, 10);
    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    let mut buf = [0u8; 64];
    p.read_at(&mut buf, 10 * piece_len).expect("read_at");
    let (code, raw) = eng.dispatch("GET", &format!("/preview-state/{id}?fileIndex=0"), "");
    assert_eq!(
        code,
        200,
        "live state: {code} {}",
        String::from_utf8_lossy(&raw)
    );
    let state: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(state["active"].as_bool().unwrap_or(false));
    assert!(state["byteOffset"].as_i64().unwrap_or(0) >= 10 * piece_len);
    p.close();
}

#[ignore = "needs: playback handle surviving download completion"]
#[test]
fn test_playback_handle_survives_download_completion() {
    // Port of Go `TestPlaybackHandleSurvivesDownloadCompletion`.
    let (eng, id, data) = support::playback_fixture_mode(false);
    let p = eng.server.open_playback(&id, 1).expect("open_playback");
    support::write_verified_piece(&eng, &id, &data, 0);
    support::write_verified_piece(&eng, &id, &data, 1);
    let mut small = [0u8; 4];
    p.read_at(&mut small, 0).expect("read_at small");
    let num_pieces = {
        let records = eng.server.records.read();
        let rec = records.get(&id).unwrap().read();
        rec.torrent.num_pieces()
    };
    for i in 2..num_pieces {
        support::write_verified_piece(&eng, &id, &data, i as usize);
    }
    let (code, _) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200);
    let want = &data[5..];
    let mut got = vec![0u8; want.len()];
    let n = p.read_at(&mut got, 0).expect("read_at after complete");
    assert_eq!(n, want.len());
    assert_eq!(&got[..], want);
    p.close();
}

#[ignore = "needs: preview does not demand missing pieces alongside player"]
#[test]
fn test_playing_preview_does_not_demand_missing_pieces() {
    // Port of Go `TestPlayingPreviewDoesNotDemandMissingPieces`.
    let (eng, id, piece_len, _data) = support::large_playback_fixture_mode(24, true);
    let player = eng.server.open_playback(&id, 0).expect("open_playback");
    let preview = eng.server.open_preview(&id, 0).expect("open_preview");
    let mut buf = [0u8; 64];
    let res = preview.read_at(&mut buf, 10 * piece_len);
    assert!(res.is_err(), "missing cached range must be unavailable");
    preview.close();
    player.close();
}
