//! Playback and peer policy for the Webtor Rust engine.
//!
//! Pure decision logic ported from `engine-go` adaptive buffer, video startup
//! hints, playback grid/resume/preload, peer exploration, Ghost Mode / Ghost
//! Pro, bypass write-split, transfer rates, idle Direct Play, and seek
//! latency helpers. `wt-engine` calls these with explicit inputs (time, sizes,
//! rates, peer snapshots, settings) and applies the returned decisions.
//!
//! See `engine-rs/docs/PORT_MAP.md` and `PORT_NOTES.md` in this crate.

// Module and type docs explain engine call sites; field docs are optional.

pub mod adaptive_buffer;
pub mod bypass;
pub mod constants;
pub mod ghost_mode;
pub mod ghost_pro;
pub mod idle_direct_play;
pub mod next_preload;
pub mod peer_exploration;
pub mod piece_span;
pub mod playback_grid;
pub mod playback_resume;
pub mod seek_latency;
pub mod seek_log;
pub mod transfer_rates;
pub mod video_priority;

pub use adaptive_buffer::AdaptiveBuffer;
pub use bypass::{
    bypass_tcp_coverage, is_loopback_addr, split_write_sync, BypassController, BypassStatus,
    BypassStream, BYPASS_MODE_OFF, BYPASS_MODE_TCP_SPLIT,
};
pub use constants::*;
pub use ghost_mode::{clamp_dial_base, GhostDialController, GhostObservation};
pub use ghost_pro::{
    compute_wanted_pieces, contiguous_wanted_pieces, peer_overlap, GhostProPeerObservation,
    GhostProSwarm, WantedContext, WantedFile,
};
pub use idle_direct_play::{
    any_playback_buffer_filling, playback_window_filling, IdleAction, IdleDirectPlay, IdleTickInput,
};
pub use next_preload::{NextPreload, NextPreloadView, PreloadFile, PreloadStartGate};
pub use peer_exploration::{PeerExploration, PeerExplorationObservation, PeerKey};
pub use piece_span::{
    file_piece_range, raise_span, span_from_file, union_span, FileLayout, PieceSpan, PlaybackWindow,
};
pub use playback_grid::{
    playback_detail, playback_focus_window, playback_tiers, range_start, split_urgent,
    PlaybackDetail, PlaybackPieceRange,
};
pub use playback_resume::{ResumeHold, ResumePrepareArgs, ResumeRange};
pub use seek_latency::{
    choke_timeout, claude_seek_read_attempts, drop_chokers, urgent_steal_after, ChokedPeer,
    ClaudeSeekTuning, SeekLatencyStats, SeekStats,
};
pub use seek_log::{pick_seek_variant, SeekLog, SeekRecord, StreamMeter, SEEK_LOG_HEADER};
pub use transfer_rates::TransferRates;
pub use video_priority::{
    is_video_file_name, video_startup_hints, VideoFileInput, VideoHintContext,
};
