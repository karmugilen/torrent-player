//! Ports of engine-go/pieces_stream_test.go

mod support;

/// Go Direct Play piece grid uses engine `/pieces` HTTP + `streamPieceRuns`.
/// wt-storage does not yet expose `stream_piece_runs` publicly.

#[test]
#[ignore = "waiting for agy-3: DocumentTorrentStorage::stream_piece_runs + piece grid hooks"]
fn test_direct_play_piece_grid_tracks_ram_replacement() {}

#[test]
#[ignore = "waiting for agy-3: DocumentTorrentStorage::stream_piece_runs"]
fn test_stream_piece_grid_partial_replacement_and_close() {
    let _ = support::FIXTURE_ROOT;
}
