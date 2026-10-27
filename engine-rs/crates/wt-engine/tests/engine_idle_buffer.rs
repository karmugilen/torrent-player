//! Integration tests ported from `engine-go/idle_buffer_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;

#[ignore = "needs: playing reader idle window management"]
#[test]
fn test_playing_reader_keeps_window_and_peers_while_idle() {
    // Port of Go `TestPlayingReaderKeepsWindowAndPeersWhileIdle`.
    let (eng, id, _piece_len, data) = support::large_playback_fixture_mode(48, true);
    let s = eng.post_settings(r#"{"ghostProMode":true,"idleDropMinutes":5}"#);
    assert!(s.ghost_pro_mode);
    support::write_verified_piece(&eng, &id, &data, 10);

    let playing = eng
        .server
        .open_playback(&id, 0)
        .expect("open_playback playing");
    let mut buf = [0u8; 4];
    let n = playing.read_at(&mut buf, 10 << 20).expect("read_at");
    assert_eq!(n, 4);

    let probe = eng
        .server
        .open_playback(&id, 0)
        .expect("open_playback probe");
    probe.close();
    playing.close();
}

#[ignore = "needs: former playhead parking when no longer playing"]
#[test]
fn test_former_playhead_parks_once_it_is_no_longer_playing() {
    // Port of Go `TestFormerPlayheadParksOnceItIsNoLongerPlaying`.
    let (eng, id, _piece_len, data) = support::large_playback_fixture_mode(64, true);
    let _ = eng.post_settings(r#"{"ghostProMode":true}"#);
    support::write_verified_piece(&eng, &id, &data, 20);
    support::write_verified_piece(&eng, &id, &data, 40);

    let playing = eng.server.open_playback(&id, 0).expect("open_playback 1");
    let next = eng.server.open_playback(&id, 0).expect("open_playback 2");
    playing.close();
    next.close();
}
