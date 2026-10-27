//! Idle Direct Play rest / drop-ready policy (`engine-go/direct_play_idle.go`).
//!
//! The engine calls [`IdleDirectPlay::tick`] once a second with injected `now`.
//! Actions tell the engine to rest (close peer slots, keep dial addrs) or wake.
//! [`playback_window_filling`] gates rest while urgent/readahead pieces are missing.

use std::time::{Duration, Instant};

use crate::constants::DIRECT_PLAY_REST_AFTER;
use crate::piece_span::PlaybackWindow;
use wt_types::PieceIndex;

/// Per-torrent idle Direct Play state.
#[derive(Clone, Debug, Default)]
pub struct IdleDirectPlay {
    last_activity: Option<Instant>,
    reading: i32,
    resting: bool,
    drop_ready: bool,
}

/// What the engine should do after a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleAction {
    None,
    /// Close peer connections; remember dial addresses (engine side).
    Rest,
    /// Restore connection cap / re-add peers.
    Wake,
}

#[derive(Clone, Debug)]
pub struct IdleTickInput {
    pub eligible: bool,
    /// Idle drop limit from settings; `None` or zero disables.
    pub idle_limit: Option<Duration>,
    pub open_readers: usize,
    pub buffer_filling: bool,
}

impl IdleDirectPlay {
    pub fn resting(&self) -> bool {
        self.resting
    }

    pub fn drop_ready(&self) -> bool {
        self.drop_ready
    }

    pub fn touch(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    pub fn begin_read(&mut self, now: Instant) -> IdleAction {
        self.reading += 1;
        self.touch(now);
        self.drop_ready = false;
        if self.resting {
            IdleAction::Wake
        } else {
            IdleAction::None
        }
    }

    /// Pair with [`Self::begin_read`]. Engine must call after the read finishes.
    pub fn end_read(&mut self, now: Instant) {
        self.touch(now);
        self.reading -= 1;
        if self.reading < 0 {
            self.reading = 0;
        }
    }

    /// Mark woken (engine finished restoring peers). Clears resting.
    pub fn mark_awake(&mut self) {
        self.resting = false;
    }

    /// Mark rested after the engine closed peer slots.
    pub fn mark_resting(&mut self) {
        self.resting = true;
    }

    pub fn tick(&mut self, now: Instant, input: &IdleTickInput) -> IdleAction {
        let limit = input.idle_limit.unwrap_or(Duration::ZERO);
        if !input.eligible || limit.is_zero() {
            let action = if self.resting {
                IdleAction::Wake
            } else {
                IdleAction::None
            };
            self.resting = false;
            self.drop_ready = false;
            self.last_activity = None;
            return action;
        }
        if self.last_activity.is_none() {
            self.touch(now);
        }
        if self.reading > 0 {
            return IdleAction::None;
        }
        if input.buffer_filling {
            self.drop_ready = false;
            return IdleAction::None;
        }
        let idle = now.duration_since(self.last_activity.unwrap_or(now));
        let rest_after = DIRECT_PLAY_REST_AFTER.min(limit);
        let mut action = IdleAction::None;
        if idle >= rest_after && !self.resting {
            self.resting = true;
            action = IdleAction::Rest;
        }
        self.drop_ready = self.resting && idle >= limit && input.open_readers == 0;
        action
    }
}

/// True when urgent or readahead still misses pieces (lookahead ignored).
pub fn playback_window_filling(
    window: &PlaybackWindow,
    mut complete: impl FnMut(PieceIndex) -> bool,
) -> bool {
    for span in [window.urgent, window.readahead] {
        for i in span.begin..span.end {
            if !complete(i) {
                return true;
            }
        }
    }
    false
}

/// Any open window still filling.
pub fn any_playback_buffer_filling(
    windows: &[PlaybackWindow],
    mut complete: impl FnMut(PieceIndex) -> bool,
) -> bool {
    windows
        .iter()
        .any(|w| playback_window_filling(w, &mut complete))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::piece_span::PieceSpan;

    #[test]
    fn rest_waits_for_readahead_not_lookahead() {
        let window = PlaybackWindow {
            urgent: PieceSpan::new(10, 12),
            readahead: PieceSpan::new(12, 20),
            lookahead: PieceSpan::new(20, 30),
            ..Default::default()
        };
        let mut complete = std::collections::HashSet::new();
        for i in 10..20 {
            complete.insert(i);
        }
        assert!(!playback_window_filling(&window, |i| complete.contains(&i)));
        complete.remove(&14);
        assert!(playback_window_filling(&window, |i| complete.contains(&i)));
    }

    #[test]
    fn rests_wakes_and_becomes_drop_ready() {
        let mut st = IdleDirectPlay::default();
        let start = Instant::now();
        let limit = Duration::from_secs(5 * 60);
        let input = IdleTickInput {
            eligible: true,
            idle_limit: Some(limit),
            open_readers: 0,
            buffer_filling: false,
        };
        assert_eq!(st.tick(start, &input), IdleAction::None);
        assert_eq!(
            st.tick(
                start + DIRECT_PLAY_REST_AFTER + Duration::from_secs(1),
                &input
            ),
            IdleAction::Rest
        );
        assert!(st.resting());
        assert!(!st.drop_ready());

        // Full idle limit while resting → drop ready.
        st.tick(start + limit + Duration::from_secs(1), &input);
        assert!(st.drop_ready());

        let wake = st.begin_read(start + limit + Duration::from_secs(2));
        assert_eq!(wake, IdleAction::Wake);
        st.mark_awake();
        st.end_read(start + limit + Duration::from_secs(2));
        assert!(!st.resting());
        assert!(!st.drop_ready());
    }

    #[test]
    fn buffer_filling_blocks_rest() {
        let mut st = IdleDirectPlay::default();
        let start = Instant::now();
        let input = IdleTickInput {
            eligible: true,
            idle_limit: Some(Duration::from_secs(300)),
            open_readers: 1,
            buffer_filling: true,
        };
        st.touch(start);
        assert_eq!(
            st.tick(
                start + DIRECT_PLAY_REST_AFTER + Duration::from_secs(1),
                &input
            ),
            IdleAction::None
        );
        assert!(!st.resting());
    }
}
