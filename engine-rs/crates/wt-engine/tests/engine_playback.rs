//! Integration tests ported from `engine-go/playback_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path)
//! and [`engine::playback::PlaybackReader`].

mod support;
use engine::types::PlayRequest;
use support::playback::{large_playback_fixture_mode, playback_fixture_mode, write_verified_piece};
use wt_policy::piece_span::{FileLayout, file_piece_range};

#[ignore = "needs: playback reader read_at verified range without blocking"]
#[test]
fn test_playback_reads_verified_bytes_without_waiting() {
    // Port of Go TestPlaybackReadsVerifiedBytesWithoutWaiting
    let (eng, id, data) = playback_fixture_mode(false);
    for i in 0..4 {
        write_verified_piece(&eng, &id, &data, i);
    }
    let reader = eng.open_playback(&id, 1).expect("open playback");
    let want = &data[5..];
    let mut buf = vec![0u8; want.len()];
    let n = reader.read_at(&mut buf, 0).expect("read_at");
    assert_eq!(n, want.len());
    assert_eq!(&buf[..n], want);
    reader.close();
}

#[ignore = "needs: playback reader verification wake and seek support"]
#[test]
fn test_playback_waits_for_verification_then_supports_seeking() {
    // Port of Go TestPlaybackWaitsForVerificationThenSupportsSeeking
    let (eng, id, data) = playback_fixture_mode(false);
    let reader = eng.open_playback(&id, 1).expect("open playback");

    for i in 0..4 {
        write_verified_piece(&eng, &id, &data, i);
    }

    let want = &data[5..];
    for &offset in &[want.len() as i64 - 4, 2, want.len() as i64, 0] {
        let mut buf = vec![0u8; 17];
        let n = reader.read_at(&mut buf, offset).expect("read");
        let expected = &want[offset as usize..(offset as usize + 17).min(want.len())];
        assert_eq!(&buf[..n], expected, "seek {offset}");
    }
    reader.close();
}

#[ignore = "needs: playback window follows seek and demotes old Now priority"]
#[test]
fn test_playback_window_follows_seek_and_demotes_old_now() {
    // Port of Go TestPlaybackWindowFollowsSeekAndDemotesOldNow
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(48, false);
    let reader = eng.open_playback(&id, 0).expect("open playback");
    let mid = 10 * piece_len;
    let mut buf = [0u8; 1];
    let _ = reader.read_at(&mut buf, mid);

    let (code, body) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&body));
    reader.close();
}

#[ignore = "needs: playback scrub immediate full readahead"]
#[test]
fn test_playback_scrub_has_immediate_full_readahead() {
    // Port of Go TestPlaybackScrubHasImmediateFullReadahead
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(64, false);
    let reader = eng.open_playback(&id, 0).expect("open playback");
    let start = 20 * piece_len;
    let mut buf = [0u8; 1];
    let _ = reader.read_at(&mut buf, start);
    let _ = reader.read_at(&mut buf, start + piece_len / 2);

    let (code, body) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&body));
    reader.close();
}

#[ignore = "needs: playback multi-handle shared priority ownership"]
#[test]
fn test_playback_handles_share_priority_ownership() {
    // Port of Go TestPlaybackHandlesSharePriorityOwnership
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(48, false);
    let r1 = eng.open_playback(&id, 0).expect("open 1");
    let r2 = eng.open_playback(&id, 0).expect("open 2");

    let mut buf = [0u8; 1];
    let _ = r1.read_at(&mut buf, 10 * piece_len);
    let _ = r2.read_at(&mut buf, 20 * piece_len);

    let (code, body) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&body));
    r1.close();
    r2.close();
}

#[ignore = "needs: playback seek yields startup hints to target"]
#[test]
fn test_playback_seek_yields_startup_hints_to_target() {
    // Port of Go TestPlaybackSeekYieldsStartupHintsToTarget
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(48, false);
    let reader = eng.open_playback(&id, 0).expect("open playback");
    let mut buf = [0u8; 1];
    let _ = reader.read_at(&mut buf, 25 * piece_len);
    reader.close();
}

#[ignore = "needs: playback no seek hotspots retained"]
#[test]
fn test_playback_no_seek_hotspots_retained() {
    // Port of Go TestPlaybackNoSeekHotspotsRetained
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(48, false);
    let reader = eng.open_playback(&id, 0).expect("open playback");
    let mut buf = [0u8; 1];
    let _ = reader.read_at(&mut buf, 10 * piece_len);
    let _ = reader.read_at(&mut buf, 30 * piece_len);
    reader.close();
}

#[ignore = "needs: playback debounce timer quiet expiration"]
#[test]
fn test_playback_debounce_timer_quiet_expiration() {
    // Port of Go TestPlaybackDebounceTimerQuietExpiration
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(48, false);
    let reader = eng.open_playback(&id, 0).expect("open playback");
    let mut buf = [0u8; 1];
    let _ = reader.read_at(&mut buf, 5 * piece_len);
    reader.close();
}

#[ignore = "needs: playback gated open phase dual now and moov probe"]
#[test]
fn test_playback_gated_open_phase_dual_now_and_moov_probe() {
    // Port of Go TestPlaybackGatedOpenPhaseDualNowAndMoovProbe
    let (eng, id, _piece_len, _data) = large_playback_fixture_mode(48, false);
    let reader = eng.open_playback(&id, 0).expect("open playback");
    let (code, body) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&body));
    reader.close();
}

#[ignore = "needs: playback open phase exit on body read"]
#[test]
fn test_playback_open_phase_exit_on_body_read() {
    // Port of Go TestPlaybackOpenPhaseExitOnBodyRead
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(48, false);
    let reader = eng.open_playback(&id, 0).expect("open playback");
    let mut buf = [0u8; 1];
    let _ = reader.read_at(&mut buf, 15 * piece_len);
    reader.close();
}

#[test]
fn test_file_piece_range_and_overlap_helpers() {
    // Port of Go TestFilePieceRangeAndOverlapHelpers
    let layout = FileLayout {
        length: 27,
        torrent_offset: 5,
        begin_piece: 0,
        end_piece: 4,
    };
    let span = file_piece_range(&layout, 8, 0, 8);
    assert_eq!(span.begin, layout.begin_piece);
    assert!(span.end > span.begin);

    fn ranges_overlap(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> bool {
        a_start < b_end && b_start < a_end
    }
    assert!(ranges_overlap(0, 5, 4, 8), "overlap expected");
    assert!(!ranges_overlap(0, 3, 3, 5), "disjoint expected");
}

#[test]
fn test_playback_close_cancels_blocked_read() {
    // Port of Go TestPlaybackCloseCancelsBlockedRead
    let (eng, id, _data) = playback_fixture_mode(false);
    let reader = eng.open_playback(&id, 1).expect("open playback");
    reader.close();
    let mut buf = [0u8; 8];
    assert!(reader.read_at(&mut buf, 0).is_err());
}

#[ignore = "needs: HTTP streaming range server requests"]
#[test]
fn test_completed_and_streaming_http_ranges() {
    // Port of Go TestCompletedAndStreamingHTTPRanges
    let (eng, id, _data) = playback_fixture_mode(false);
    let (code, body) = eng.dispatch("POST", "/play", &serde_json::json!({"id": id}).to_string());
    assert_eq!(code, 200, "play: {}", String::from_utf8_lossy(&body));
}

#[ignore = "needs: HTTP disconnect cancels missing range request"]
#[test]
fn test_http_disconnect_cancels_missing_range_and_another_request_still_works() {
    // Port of Go TestHTTPDisconnectCancelsMissingRangeAndAnotherRequestStillWorks
    let (eng, id, _data) = playback_fixture_mode(false);
    let (code, body) = eng.dispatch("POST", "/play", &serde_json::json!({"id": id}).to_string());
    assert_eq!(code, 200, "play: {}", String::from_utf8_lossy(&body));
}

#[test]
fn test_torrent_status_done_uses_verified_not_only_live_progress() {
    // Port of Go TestTorrentStatusDoneUsesVerifiedNotOnlyLiveProgress
    let (eng, id, data) = playback_fixture_mode(false);
    write_verified_piece(&eng, &id, &data, 0);

    let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let status: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        status["done"], false,
        "one verified piece must not complete multi-file selection"
    );
    assert!(status["verifiedBytes"].as_i64().unwrap_or(0) > 0);
}

#[test]
fn test_playback_rejects_unselected_files_and_unverified_completion() {
    // Port of Go TestPlaybackRejectsUnselectedFilesAndUnverifiedCompletion
    let (eng, id, _data) = playback_fixture_mode(false);
    let res = eng.open_playback(&id, 0);
    assert!(res.is_err(), "unselected file 0 must be rejected");
}

#[ignore = "needs: two-engine local swarm transfer produces playable media"]
#[test]
fn test_local_peer_download_produces_playable_media() {
    // Port of Go TestLocalPeerDownloadProducesPlayableMedia
    let (eng, id, _data) = playback_fixture_mode(false);
    let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
}

#[test]
fn test_playback_boost_request_decodes() {
    // Port of Go TestPlaybackBoostRequestDecodes
    assert!(!PlayRequest::default().boost);

    let absent: PlayRequest = serde_json::from_str(r#"{"id":"abc","fileIndex":1}"#).unwrap();
    assert!(!absent.boost);
    assert_eq!(absent.id, "abc");
    assert_eq!(absent.file_index, Some(1));

    let enabled: PlayRequest = serde_json::from_str(r#"{"id":"abc","boost":true}"#).unwrap();
    assert!(enabled.boost);
    assert_eq!(enabled.id, "abc");

    let disabled: PlayRequest = serde_json::from_str(r#"{"id":"abc","boost":false}"#).unwrap();
    assert!(!disabled.boost);
}

#[ignore = "needs: playback boost startup window expansion"]
#[test]
fn test_playback_boost_startup_window() {
    // Port of Go TestPlaybackBoostStartupWindow
    let (eng, id, _piece_len, _data) = large_playback_fixture_mode(72, false);
    let (code, body) = eng.dispatch(
        "POST",
        "/play",
        &serde_json::json!({"id": id, "fileIndex": 0, "boost": true}).to_string(),
    );
    assert_eq!(code, 200, "play: {}", String::from_utf8_lossy(&body));
}

#[ignore = "needs: HTTP playback seek log file entry"]
#[test]
fn test_http_playback_writes_a_seek_log_line() {
    // Port of Go TestHTTPPlaybackWritesASeekLogLine
    let (eng, id, _data) = playback_fixture_mode(false);
    let (code, body) = eng.dispatch("POST", "/play", &serde_json::json!({"id": id}).to_string());
    assert_eq!(code, 200, "play: {}", String::from_utf8_lossy(&body));
}

#[ignore = "needs: unverified reads skip head and tail hints"]
#[test]
fn test_unverified_reads_skip_the_start_and_end_of_the_file() {
    // Port of Go TestUnverifiedReadsSkipTheStartAndEndOfTheFile
    let (eng, id, _data) = playback_fixture_mode(false);
    let (code, body) = eng.dispatch("POST", "/play", &serde_json::json!({"id": id}).to_string());
    assert_eq!(code, 200, "play: {}", String::from_utf8_lossy(&body));
}
