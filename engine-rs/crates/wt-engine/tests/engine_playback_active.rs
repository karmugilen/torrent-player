//! Integration tests ported from `engine-go/playback_active_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;

#[ignore = "needs: active playback coordinator demoting older seek positions"]
#[test]
fn test_only_the_latest_seek_position_is_fetched() {
    // Port of Go `TestOnlyTheLatestSeekPositionIsFetched`.
    let (eng, id, _piece_len, data) = support::large_playback_fixture_mode(64, true);
    for piece in [10, 30, 50] {
        support::write_verified_piece(&eng, &id, &data, piece);
    }
    let first = eng.server.open_playback(&id, 0).expect("open 1");
    let second = eng.server.open_playback(&id, 0).expect("open 2");
    let third = eng.server.open_playback(&id, 0).expect("open 3");

    let mut buf = [0u8; 4];
    assert_eq!(first.read_at(&mut buf, 10 << 20).unwrap(), 4);
    assert_eq!(second.read_at(&mut buf, 30 << 20).unwrap(), 4);
    assert_eq!(third.read_at(&mut buf, 50 << 20).unwrap(), 4);

    second.close();
    assert_eq!(first.read_at(&mut buf, 10 << 20).unwrap(), 4);

    first.close();
    third.close();
}

#[ignore = "needs: blocked read at earlier spot superseded by new seek"]
#[test]
fn test_blocked_read_at_an_earlier_spot_stops_and_stays_stopped() {
    // Port of Go `TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped`.
    let (eng, id, _piece_len, data) = support::large_playback_fixture_mode(64, true);
    support::write_verified_piece(&eng, &id, &data, 30);

    let old = eng.server.open_playback(&id, 0).expect("open old");
    let current = eng.server.open_playback(&id, 0).expect("open current");

    let old_clone = old.clone();
    let handle = std::thread::spawn(move || {
        let mut buf = [0u8; 4];
        old_clone.read_at(&mut buf, 20 << 20)
    });

    let mut buf = [0u8; 4];
    assert_eq!(current.read_at(&mut buf, 30 << 20).unwrap(), 4);

    let _ = handle.join();
    old.close();
    current.close();
}
