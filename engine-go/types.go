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
}

type PlayRequest struct {
	ID        string `json:"id"`
	FileIndex *int   `json:"fileIndex,omitempty"`
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
// (MediaStore / SAF). StorageModeStream writes selected files to app-private
// engine files under .stream/ and never expects external descriptors.
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
	// StorageMode is "download" (default) or "stream". Stream mode opens
	// selected files under .stream/ with nil descriptors.
	StorageMode string `json:"storageMode,omitempty"`
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
}

type MetadataResponse struct {
	TorrentData string `json:"torrentData"`
}

type SettingsResponse struct {
	MaxPeers             int   `json:"maxPeers"`
	DownloadRateBytesSec int64 `json:"downloadRateBytesSec"`
	UploadRateBytesSec   int64 `json:"uploadRateBytesSec"`
	// PayloadTransfersAllowed controls torrent file bytes only. Tracker, DHT,
	// PEX, and metadata discovery keep running while it is false.
	PayloadTransfersAllowed bool `json:"payloadTransfersAllowed"`
}

type SettingsRequest struct {
	MaxPeers             int    `json:"maxPeers"`
	DownloadRateBytesSec *int64 `json:"downloadRateBytesSec,omitempty"`
	UploadRateBytesSec   *int64 `json:"uploadRateBytesSec,omitempty"`
	// A pointer distinguishes an omitted setting from an explicit false.
	PayloadTransfersAllowed *bool `json:"payloadTransfersAllowed,omitempty"`
}

type OkResponse struct {
	Ok bool   `json:"ok"`
	ID string `json:"id,omitempty"`
}
