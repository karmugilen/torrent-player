//! Data models and JSON views serialized between the engine and Kotlin/HTTP clients.
//!
//! All fields match `engine-go/types.go` and Kotlin `EngineClient.kt` byte-for-byte.

use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !value
}

fn empty_string(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(String::is_empty)
}

fn empty_vec<T>(value: &Option<Vec<T>>) -> bool {
    value.as_ref().is_none_or(Vec::is_empty)
}

fn zero_option(value: &Option<i64>) -> bool {
    value.is_none_or(|v| v == 0)
}

fn serialize_max_peers<S: serde::Serializer>(
    value: &Option<usize>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(value.unwrap_or(0) as u64)
}

fn serialize_go_float<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if !value.is_finite() {
        return Err(serde::ser::Error::custom("unsupported non-finite float"));
    }
    if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value < i64::MAX as f64 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsResponse {
    pub download_speed: i64,
    pub upload_speed: i64,
    #[serde(serialize_with = "serialize_go_float")]
    pub progress: f64,
    #[serde(serialize_with = "serialize_go_float")]
    pub ratio: f64,
    pub torrents: usize,
    pub ctl_port: u16,
    pub stream_port: u16,
    pub path: String,
    pub payload_transfers_allowed: bool,
    pub listen_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    #[serde(default, alias = "torrent_id")]
    pub torrent_id: String,
    #[serde(default)]
    pub prepare: bool,
    #[serde(default, skip_serializing_if = "empty_string")]
    pub torrent_data: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddResponse {
    pub id: String,
    pub info_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub index: usize,
    pub name: String,
    pub path: String,
    pub length: i64,
    #[serde(serialize_with = "serialize_go_float")]
    pub progress: f64,
    pub verified_bytes: i64,
    #[serde(rename = "type")]
    pub file_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerView {
    pub url: String,
    pub tier: usize,
    pub status: String,
    #[serde(skip_serializing_if = "empty_string")]
    pub message: Option<String>,
    pub original: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextPreloadView {
    pub file_index: usize,
    pub active: bool,
    pub ready: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentView {
    pub seeding: bool,
    #[serde(serialize_with = "serialize_go_float")]
    pub seed_ratio: f64,
    pub id: String,
    pub info_hash: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "magnetURI")]
    pub magnet_uri: Option<String>,
    pub ready: bool,
    pub done: bool,
    pub paused: bool,
    #[serde(serialize_with = "serialize_go_float")]
    pub progress: f64,
    pub download_speed: i64,
    pub upload_speed: i64,
    pub num_peers: usize,
    pub length: i64,
    pub downloaded: i64,
    pub verified_bytes: i64,
    pub uploaded: i64,
    pub files: Vec<FileView>,
    pub trackers: Vec<TrackerView>,
    pub error: Option<String>,
    pub time_remaining: Option<i64>,
    pub configured: bool,
    pub selected: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_file: Option<usize>,
    pub checking: bool,
    pub checked_pieces: usize,
    pub check_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub playback_readers: usize,
    pub resting: bool,
    pub idle_drop_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_preload: Option<NextPreloadView>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(default)]
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_index: Option<usize>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub boost: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub file_only: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub adaptive_buffer: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub burst_aware_buffer: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_file_index: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusRequest {
    #[serde(default)]
    pub id: String,
    pub file_index: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayResponse {
    pub id: String,
    pub file_index: usize,
    pub name: String,
    pub length: i64,
    pub stream_url: String,
}

pub const STORAGE_MODE_DOWNLOAD: &str = "download";
pub const STORAGE_MODE_STREAM: &str = "stream";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureRequest {
    pub id: String,
    pub descriptors: Option<Vec<Option<i32>>>,
    pub selected: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "empty_string")]
    pub storage_mode: Option<String>,
    #[serde(skip_serializing_if = "zero_option")]
    pub stream_cache_bytes: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectRequest {
    pub id: String,
    pub selected: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "empty_vec")]
    pub descriptors: Option<Vec<Option<i32>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectResponse {
    pub ok: bool,
    pub selected: Option<Vec<usize>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdRequest {
    #[serde(default)]
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destroy_store: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PieceBucket {
    pub start: usize,
    pub end: usize,
    pub total: usize,
    pub selected: usize,
    pub verified: usize,
    pub receiving: usize,
    pub selected_verified: usize,
    pub selected_receiving: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackDetail {
    pub start: usize,
    pub states: String,
    pub playhead: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackPieceRange {
    pub start: usize,
    pub end: usize,
    pub priority: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PieceTelemetryResponse {
    pub id: String,
    pub info_hash: Option<String>,
    pub generation: i64,
    pub timestamp: i64,
    pub total_pieces: usize,
    pub piece_length: i64,
    pub last_piece_length: i64,
    pub max_buckets: usize,
    pub buckets: Vec<PieceBucket>,
    pub ghost_pro_mode: bool,
    #[serde(skip_serializing_if = "empty_vec")]
    pub playback: Option<Vec<PlaybackPieceRange>>,
    #[serde(skip_serializing_if = "empty_vec")]
    pub playheads: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_detail: Option<PlaybackDetail>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataResponse {
    pub torrent_data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SeekStats {
    pub count: usize,
    pub last_ms: i64,
    pub median_ms: i64,
    pub stalls: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsResponse {
    pub ghost_mode: bool,
    pub ghost_pro_mode: bool,
    pub dial_rate_per_sec: usize,
    pub effective_dial_rate_per_sec: i64,
    pub peer_exploration: bool,
    pub claude_seek_mode: bool,
    pub hedge_requests: bool,
    pub slow_peer_yield: bool,
    pub drop_chokers: bool,
    pub deadline_seek: bool,
    pub idle_drop_minutes: i64,
    pub ram_cache_bytes: i64,
    pub seeding_enabled: bool,
    pub seed_ratio_milli: i64,
    pub seed_time_minutes: i64,
    pub max_peers: usize,
    pub download_rate_bytes_sec: i64,
    pub upload_rate_bytes_sec: i64,
    pub payload_transfers_allowed: bool,
    pub seek_stats: SeekStats,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SettingsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ghost_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ghost_pro_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_exploration: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_seek_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hedge_requests: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slow_peer_yield: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_chokers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_seek: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_drop_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_cache_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeding_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeding_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_ratio_milli: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_time_minutes: Option<i64>,
    #[serde(default, serialize_with = "serialize_max_peers")]
    pub max_peers: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_rate_bytes_sec: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_rate_bytes_sec: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_transfers_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dial_rate_per_sec: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seek_log_path: Option<String>,
    #[serde(default, skip_serializing_if = "empty_string")]
    pub seek_log_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seek_experiment: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "empty_string")]
    pub id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOnlyResponse {
    pub file_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetPeersRequest {
    pub info_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClearCacheRequest {
    pub categories: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerConnectionView {
    pub address: String,
    pub transport: String,
    pub client: String,
    pub downloaded: i64,
    pub uploaded: i64,
    #[serde(skip_serializing_if = "empty_string")]
    pub country: Option<String>,
    /// Remote is choking us (swarm diagnostics; ignored by older clients).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_choking: Option<bool>,
    /// We sent Interested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub am_interested: Option<bool>,
    /// Outstanding block requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_requests: Option<usize>,
    /// Estimated peer completion 0.0–1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_completion: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nominal_max_requests: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_max_requests: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_requests: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsResponse {
    pub id: String,
    pub paused: bool,
    pub peers: Vec<PeerConnectionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<wt_core::stats::SwarmDiagnostics>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStateResponse {
    pub active: bool,
    pub byte_offset: i64,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkChangeRequest {
    #[serde(default)]
    pub generation: i64,
    pub connected: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BypassStatus {
    pub enabled: bool,
    pub active: bool,
    pub mode: String,
    pub coverage: Option<Vec<String>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheCategoryView {
    pub id: String,
    pub bytes: i64,
    pub reclaimable_bytes: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStatusResponse {
    pub categories: Vec<CacheCategoryView>,
    pub memory_bytes: i64,
}
