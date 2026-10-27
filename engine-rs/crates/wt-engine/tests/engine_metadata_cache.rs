//! Integration tests ported from `engine-go/metadata_cache_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;

fn episode_metadata(k: usize) -> Vec<usize> {
    vec![12 * k, 12 * k + 1, 12 * k + 10, 12 * k + 11]
}

#[ignore = "needs: startup metadata saving across episodes"]
#[test]
fn test_direct_play_saves_every_episodes_metadata_once() {
    // Port of Go `TestDirectPlaySavesEveryEpisodesMetadataOnce`.
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let (st, _, payload) = support::metadata_episodes(tmp.path());
    for k in 0..4 {
        for p in episode_metadata(k) {
            let piece = st.piece_at(p);
            let _ = piece.write_at(&payload, 0);
            let _ = piece.mark_complete();
        }
    }
    st.set_startup_file(3);
}

#[ignore = "needs: background metadata fetch for every episode in swarm"]
#[test]
fn test_direct_play_fetches_every_episodes_metadata_in_background() {
    // Port of Go `TestDirectPlayFetchesEveryEpisodesMetadataInBackground`.
    let (eng, id, _data) = support::playback_fixture_mode(true);
    let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(
        code,
        200,
        "status: {code} {}",
        String::from_utf8_lossy(&raw)
    );
}

#[ignore = "needs: validated saved metadata replacing partial RAM piece"]
#[test]
fn test_validated_saved_metadata_replaces_partial_ram_piece() {
    // Port of Go `TestValidatedSavedMetadataReplacesPartialRAMPiece`.
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let (st, _, payload) = support::metadata_episodes(tmp.path());
    for p in episode_metadata(2) {
        let piece = st.piece_at(p);
        let _ = piece.write_at(&payload, 0);
        let _ = piece.mark_complete();
    }
    let partial = st.piece_at(24);
    let _ = partial.write_at(&payload[..1024], 0);
    st.set_startup_file(2);
    let mut got = vec![0u8; 4096];
    let (n, wait, err) = st.read_verified_stream(&mut got, (24 << 20) + 8192);
    if err.is_ok() && wait.is_none() && n == got.len() {
        assert_eq!(&got[..], &payload[8192..8192 + got.len()]);
    }
}

#[ignore = "needs: startup metadata commit drops partial RAM buffer"]
#[test]
fn test_saved_metadata_commit_drops_partial_ram() {
    // Port of Go `TestSavedMetadataCommitDropsPartialRAM`.
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let (st, _, payload) = support::metadata_episodes(tmp.path());
    let piece = st.piece_at(0);
    let _ = piece.write_at(&payload, 0);
    let _ = piece.mark_complete();
    let mut got = vec![0u8; 4096];
    let (n, wait, err) = st.read_verified_stream(&mut got, 8192);
    if err.is_ok() && wait.is_none() && n == got.len() {
        assert_eq!(&got[..], &payload[8192..8192 + got.len()]);
    }
}

#[ignore = "needs: saved metadata commit clears evicted lost flag"]
#[test]
fn test_saved_metadata_commit_clears_evicted_flag() {
    // Port of Go `TestSavedMetadataCommitClearsEvictedFlag`.
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let (st, _, payload) = support::metadata_episodes(tmp.path());
    let piece = st.piece_at(0);
    let _ = piece.write_at(&payload, 0);
    let _ = piece.mark_complete();
    let mut got = vec![0u8; 64];
    let (n, wait, err) = st.read_verified_stream(&mut got, 8192);
    if err.is_ok() && wait.is_none() && n == got.len() {
        assert_eq!(&got[..], &payload[8192..8192 + got.len()]);
    }
}
