package main

type StatsResponse struct {
	DownloadSpeed           int64   `json:"downloadSpeed"`
	UploadSpeed             int64   `json:"uploadSpeed"`
	Progress                float64 `json:"progress"`
	Ratio                   float64 `json:"ratio"`
	Torrents                int     `json:"torrents"`
	CtlPort                 int     `json:"ctlPort"`
	StreamPort              int     `json:"streamPort"`
	Path                    string  `json:"path"`
	PayloadTransfersAllowed bool    `json:"payloadTransfersAllowed"`
	ListenPort              int     `json:"listenPort"`
}

type AddRequest struct {
	TorrentID   string `json:"torrentId"`
	Prepare     bool   `json:"prepare"`
	TorrentData string `json:"torrentData,omitempty"`
}

type AddResponse struct {
	ID       string  `json:"id"`
	InfoHash *string `json:"infoHash"`
}

type FileView struct {
	Index         int     `json:"index"`
	Name          string  `json:"name"`
	Path          string  `json:"path"`
	Length        int64   `json:"length"`
	Progress      float64 `json:"progress"`
	VerifiedBytes int64   `json:"verifiedBytes"`
	Type          string  `json:"type"`
}

type TrackerView struct {
	URL      string `json:"url"`
	Tier     int    `json:"tier"`
	Status   string `json:"status"`
	Message  string `json:"message,omitempty"`
	Original bool   `json:"original"`
}

type TorrentView struct {
	Seeding       bool          `json:"seeding"`
	SeedRatio     float64       `json:"seedRatio"`
	ID            string        `json:"id"`
	InfoHash      *string       `json:"infoHash"`
	Name          *string       `json:"name"`
	MagnetURI     *string       `json:"magnetURI"`
	Ready         bool          `json:"ready"`
	Done          bool          `json:"done"`
	Paused        bool          `json:"paused"`
	Progress      float64       `json:"progress"`
	DownloadSpeed int64         `json:"downloadSpeed"`
	UploadSpeed   int64         `json:"uploadSpeed"`
	NumPeers      int           `json:"numPeers"`
	Length        int64         `json:"length"`
	Downloaded    int64         `json:"downloaded"`
	VerifiedBytes int64         `json:"verifiedBytes"`
	Uploaded      int64         `json:"uploaded"`
	Files         []FileView    `json:"files"`
	Trackers      []TrackerView `json:"trackers"`
	Error         *string       `json:"error"`
	TimeRemaining *int64        `json:"timeRemaining"`
	Configured    bool          `json:"configured"`
	Selected      []int         `json:"selected"`
	FocusedFile   *int          `json:"focusedFile,omitempty"`
	Checking      bool          `json:"checking"`
	CheckedPieces int           `json:"checkedPieces"`
	CheckTotal    int           `json:"checkTotal"`
	Warning       *string       `json:"warning,omitempty"`
	// Direct Play only: open player readers, peers released while idle, and
	// whether Android should drop the idle session.
	PlaybackReaders int  `json:"playbackReaders"`
	Resting         bool `json:"resting"`
	IdleDropReady   bool `json:"idleDropReady"`
	// Play next warm-up of the following episode, when Android named one.
	NextPreload *NextPreloadView `json:"nextPreload,omitempty"`
}

type PlayRequest struct {
	Offset    *int64 `json:"offset,omitempty"`
	ID        string `json:"id"`
	FileIndex *int   `json:"fileIndex,omitempty"`
	// Boost enlarges only this play call's startup window. Absent is false.
	Boost bool `json:"boost,omitempty"`
	// FileOnly limits a saved download to the played file until playback ends.
	FileOnly bool `json:"fileOnly,omitempty"`
	// AdaptiveBuffer enables experimental demand-variation headroom per play.
	AdaptiveBuffer   bool `json:"adaptiveBuffer,omitempty"`
	BurstAwareBuffer bool `json:"burstAwareBuffer,omitempty"`
	// NextFileIndex is warmed near the end of this file when Play next is on.
	NextFileIndex *int `json:"nextFileIndex,omitempty"`
}

// FocusRequest changes the background download focus. A null fileIndex clears
// the focus while retaining the normal selected-file priorities.
type FocusRequest struct {
	ID        string `json:"id"`
	FileIndex *int   `json:"fileIndex"`
}

type PlayResponse struct {
	ID        string `json:"id"`
	FileIndex int    `json:"fileIndex"`
	Name      string `json:"name"`
	Length    int64  `json:"length"`
	StreamURL string `json:"streamUrl"`
}

// StorageModeDownload writes selected files to Android-supplied descriptors
// (MediaStore / SAF). StorageModeStream keeps video in bounded RAM and saves
// only verified startup pieces under .startup/, with no external descriptors.
const (
	StorageModeDownload = "download"
	StorageModeStream   = "stream"
)

type ConfigureRequest struct {
	ID string `json:"id"`
	// One entry per torrent file. A non-nil descriptor is retained even when
	// that file is not in Selected so a restart can preserve its URI-backed
	// bytes for later reselection and verified local playback.
	Descriptors []*int `json:"descriptors"`
	Selected    []int  `json:"selected"`
	// StorageMode is "download" (default) or "stream" (Direct Play).
	// Direct Play requires nil descriptors.
	StorageMode      string `json:"storageMode,omitempty"`
	StreamCacheBytes int64  `json:"streamCacheBytes,omitempty"`
}

type SelectRequest struct {
	ID       string `json:"id"`
	Selected []int  `json:"selected"`
	// Descriptors is optional for backward compatibility. It must contain one
	// entry per torrent file when a previously unselected file is added. Null
	// entries retain engine-owned storage or an already attached external file.
	Descriptors []*int `json:"descriptors,omitempty"`
}

type SelectResponse struct {
	Ok       bool  `json:"ok"`
	Selected []int `json:"selected"`
}

type IdRequest struct {
	ID           string `json:"id"`
	DestroyStore *bool  `json:"destroyStore,omitempty"`
}

type PieceBucket struct {
	Start             int `json:"start"`
	End               int `json:"end"`
	Total             int `json:"total"`
	Selected          int `json:"selected"`
	Verified          int `json:"verified"`
	Receiving         int `json:"receiving"`
	SelectedVerified  int `json:"selectedVerified"`
	SelectedReceiving int `json:"selectedReceiving"`
}

type PieceTelemetryResponse struct {
	ID              string        `json:"id"`
	InfoHash        *string       `json:"infoHash"`
	Generation      int64         `json:"generation"`
	Timestamp       int64         `json:"timestamp"`
	TotalPieces     int           `json:"totalPieces"`
	PieceLength     int64         `json:"pieceLength"`
	LastPieceLength int64         `json:"lastPieceLength"`
	MaxBuckets      int           `json:"maxBuckets"`
	Buckets         []PieceBucket `json:"buckets"`
	GhostProMode    bool          `json:"ghostProMode"`
	// Playback tiers: U this block (Now, at the playhead), N the next blocks
	// fetched in order after it. Nothing behind the playhead.
	Playback []PlaybackPieceRange `json:"playback,omitempty"`
	// Playheads holds the piece each open reader is at.
	Playheads []int `json:"playheads,omitempty"`
	// PlaybackDetail is the playing reader's window at one piece per state.
	PlaybackDetail *PlaybackDetail `json:"playbackDetail,omitempty"`
}

// PlaybackDetail lists each piece of a window from Start: v verified, s saved
// on storage only (Direct Play start and end), r receiving, m missing.
type PlaybackDetail struct {
	Start    int    `json:"start"`
	States   string `json:"states"`
	Playhead int    `json:"playhead"`
}

// Inclusive torrent piece indices, matching PieceBucket's range convention.
type PlaybackPieceRange struct {
	Start    int    `json:"start"`
	End      int    `json:"end"`
	Priority string `json:"priority"`
}

type MetadataResponse struct {
	TorrentData string `json:"torrentData"`
}

type SettingsResponse struct {
	GhostMode               bool  `json:"ghostMode"`
	GhostProMode            bool  `json:"ghostProMode"`
	DialRatePerSec          int   `json:"dialRatePerSec"`
	EffectiveDialRatePerSec int64 `json:"effectiveDialRatePerSec"`
	PeerExploration         bool  `json:"peerExploration"`
	ClaudeSeekMode          bool  `json:"claudeSeekMode"`
	HedgeRequests           bool  `json:"hedgeRequests"`
	SlowPeerYield           bool  `json:"slowPeerYield"`
	DropChokers             bool  `json:"dropChokers"`
	LearnedArrival          bool  `json:"learnedArrival"`
	// Arrival is how well block arrival times are predicted.
	Arrival              ArrivalStats `json:"arrival"`
	IdleDropMinutes      int          `json:"idleDropMinutes"`
	RamCacheBytes        int64        `json:"ramCacheBytes"`
	SeedingEnabled       bool         `json:"seedingEnabled"`
	SeedRatioMilli       int64        `json:"seedRatioMilli"`
	SeedTimeMinutes      int64        `json:"seedTimeMinutes"`
	MaxPeers             int          `json:"maxPeers"`
	DownloadRateBytesSec int64        `json:"downloadRateBytesSec"`
	UploadRateBytesSec   int64        `json:"uploadRateBytesSec"`
	// PayloadTransfersAllowed controls torrent file bytes only. Tracker, DHT,
	// PEX, and metadata discovery keep running while it is false.
	PayloadTransfersAllowed bool `json:"payloadTransfersAllowed"`
	// Seek-to-first-byte times since Claude optimized seek last changed.
	SeekStats SeekStats `json:"seekStats"`
}

type SettingsRequest struct {
	GhostMode            *bool  `json:"ghostMode,omitempty"`
	GhostProMode         *bool  `json:"ghostProMode,omitempty"`
	PeerExploration      *bool  `json:"peerExploration,omitempty"`
	ClaudeSeekMode       *bool  `json:"claudeSeekMode,omitempty"`
	HedgeRequests        *bool  `json:"hedgeRequests,omitempty"`
	SlowPeerYield        *bool  `json:"slowPeerYield,omitempty"`
	DropChokers          *bool  `json:"dropChokers,omitempty"`
	LearnedArrival       *bool  `json:"learnedArrival,omitempty"`
	IdleDropMinutes      *int   `json:"idleDropMinutes,omitempty"`
	RamCacheBytes        *int64 `json:"ramCacheBytes,omitempty"`
	SeedingEnabled       *bool  `json:"seedingEnabled,omitempty"`
	SeedingAllowed       *bool  `json:"seedingAllowed,omitempty"`
	SeedRatioMilli       *int64 `json:"seedRatioMilli,omitempty"`
	SeedTimeMinutes      *int64 `json:"seedTimeMinutes,omitempty"`
	MaxPeers             int    `json:"maxPeers"`
	DownloadRateBytesSec *int64 `json:"downloadRateBytesSec,omitempty"`
	UploadRateBytesSec   *int64 `json:"uploadRateBytesSec,omitempty"`
	// A pointer distinguishes an omitted setting from an explicit false.
	PayloadTransfersAllowed *bool `json:"payloadTransfersAllowed,omitempty"`
	// Nil leaves the dial limiter unchanged. A set value is clamped to 10..80.
	DialRatePerSec *int `json:"dialRatePerSec,omitempty"`
	// SeekLogPath names the CSV file that gets one line per HTTP playback
	// request; empty stops it. SeekLogTag (the app version) goes on each line.
	SeekLogPath *string `json:"seekLogPath,omitempty"`
	SeekLogTag  string  `json:"seekLogTag,omitempty"`
	// SeekExperiment picks the seek rules at random for each HTTP range
	// request, and the seek log names them, for A/B tests on the phone.
	SeekExperiment *bool `json:"seekExperiment,omitempty"`
}

type OkResponse struct {
	Ok bool   `json:"ok"`
	ID string `json:"id,omitempty"`
}

// ArrivalStats reports the block arrival network learned on the device and
// its running mean error.
type ArrivalStats struct {
	Samples      int64   `json:"samples"`
	NetworkErrMs float64 `json:"networkErrMs"`
	// Using is whether the network's estimate is in use.
	Using bool `json:"using"`
}
