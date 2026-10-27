//! Torrent engine coordinator, JNI layer, HTTP streaming server, and control API.
//!
//! Pure Rust port of `engine-go`. See `engine-rs/docs/PORT_MAP.md` for crate scope.

pub mod connections;
pub mod ghost;
pub mod telemetry;
pub mod peer_exploration;
pub mod ctl_http;
pub mod jni;
pub mod live_preview;
pub mod network;
pub mod playback;
pub mod playback_core;
pub mod preview;
pub mod seeding;
pub mod server;
pub mod stream_http;
pub mod tracker_status;
pub mod trackers;
pub mod types;
pub mod version;

pub use jni::host_api::EngineHostApi;
pub use server::EngineServer;
pub use version::app_version;
