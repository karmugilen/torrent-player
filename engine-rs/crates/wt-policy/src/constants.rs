//! Playback and policy constants mirrored from `engine-go/playback.go` and
//! related files. The engine passes these (or uses the defaults) when calling
//! policy helpers; they are not read from the environment.

use std::time::Duration;

/// Playhead tip fetched at `PiecePriority::Now` (~2 MiB).
pub const PLAYBACK_URGENT_BYTES: i64 = 2 * 1024 * 1024;
/// Forward readahead beyond the urgent tip (~24 MiB).
pub const PLAYBACK_READAHEAD_BYTES: i64 = 24 * 1024 * 1024;
/// Jump size that counts as a scrub (~2 MiB).
pub const PLAYBACK_SCRUB_JUMP_BYTES: i64 = 2 * 1024 * 1024;
/// Debounce after a scrub before settling the window.
pub const PLAYBACK_SCRUB_DEBOUNCE: Duration = Duration::from_millis(250);

/// Soft video startup head (~5 MiB) raised to Now.
pub const VIDEO_STARTUP_HEAD_BYTES: i64 = 5 * 1024 * 1024;
/// Soft video startup tail (~3 MiB) raised to Now.
pub const VIDEO_STARTUP_TAIL_BYTES: i64 = 3 * 1024 * 1024;

/// Ghost Pro / boost head (~16 MiB).
pub const PLAYBACK_BOOST_HEAD_BYTES: i64 = 16 * 1024 * 1024;
/// Ghost Pro / boost tail (~8 MiB).
pub const PLAYBACK_BOOST_TAIL_BYTES: i64 = 8 * 1024 * 1024;
/// Ghost Pro fat readahead (~48 MiB).
pub const PLAYBACK_BOOST_READAHEAD_BYTES: i64 = 48 * 1024 * 1024;

/// Direct Play durable startup head (~2 MiB).
pub const DIRECT_STARTUP_HEAD: i64 = 2 << 20;
/// Direct Play durable startup tail (~2 MiB).
pub const DIRECT_STARTUP_TAIL: i64 = 2 << 20;

/// Resume hold TTL (90 s).
pub const PLAYBACK_RESUME_TTL: Duration = Duration::from_secs(90);
/// Debounced resume persist delay (2 s).
pub const RESUME_PERSIST_DELAY: Duration = Duration::from_secs(2);
/// Max verified resume window bytes (4 MiB).
pub const RESUME_WINDOW_MAX_BYTES: i64 = 4 << 20;

/// Fraction of the current file at which next-episode preload may start.
pub const NEXT_PRELOAD_AT: f64 = 0.9;

/// Default dial rate (peers/s) when Ghost Mode is off.
pub const DEFAULT_DIAL_RATE: i32 = 30;
/// Shared dial ceiling with Fast peer search.
pub const MAX_DIAL_RATE: i32 = 80;

/// How long Direct Play rests before closing peer slots.
pub const DIRECT_PLAY_REST_AFTER: Duration = Duration::from_secs(2 * 60);
/// Cap on remembered dial addresses while resting.
pub const REST_PEER_LIMIT: usize = 80;

/// Seek latency rolling window size.
pub const SEEK_STATS_WINDOW: usize = 20;
/// Claude seek: steal urgent blocks after this age.
pub const CLAUDE_SEEK_STEAL_AFTER: Duration = Duration::from_millis(500);
/// Claude seek: nudge interval while a read is blocked.
pub const CLAUDE_SEEK_NUDGE_EVERY: Duration = Duration::from_millis(100);
/// Drop-chokers timeout (WebTorrent CHOKE_TIMEOUT).
pub const CHOKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Arrival model save cadence.
pub const ARRIVAL_SAVE_EVERY: Duration = Duration::from_secs(60);

/// Bypass: first N payload bytes are write-split one byte at a time.
pub const BYPASS_SPLIT_FIRST_BYTES: usize = 8;
/// Inter-fragment delay inside the split window.
pub const BYPASS_SPLIT_DELAY: Duration = Duration::from_millis(2);

/// Per-piece grid detail bound.
pub const PLAYBACK_DETAIL_MAX: usize = 2048;

/// Extra Now pieces past the 2 MiB playhead tip (seek search may raise this).
pub static URGENT_NOW_EXTRA_PIECES: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
