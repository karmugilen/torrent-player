package main

type StatsResponse struct {
	DownloadSpeed int64   `json:"downloadSpeed"`
	UploadSpeed   int64   `json:"uploadSpeed"`
	Progress      float64 `json:"progress"`
	Ratio         float64 `json:"ratio"`
	Torrents      int     `json:"torrents"`
	CtlPort       int     `json:"ctlPort"`
	StreamPort    int     `json:"streamPort"`
	Path          string  `json:"path"`
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
	Index    int     `json:"index"`
	Name     string  `json:"name"`
	Path     string  `json:"path"`
	Length   int64   `json:"length"`
	Progress float64 `json:"progress"`
	Type     string  `json:"type"`
}

type TorrentView struct {
	ID            string     `json:"id"`
	InfoHash      *string    `json:"infoHash"`
	Name          *string    `json:"name"`
	MagnetURI     *string    `json:"magnetURI"`
	Ready         bool       `json:"ready"`
	Done          bool       `json:"done"`
	Paused        bool       `json:"paused"`
	Progress      float64    `json:"progress"`
	DownloadSpeed int64      `json:"downloadSpeed"`
	UploadSpeed   int64      `json:"uploadSpeed"`
	NumPeers      int        `json:"numPeers"`
	Length        int64      `json:"length"`
	Downloaded    int64      `json:"downloaded"`
	Uploaded      int64      `json:"uploaded"`
	Files         []FileView `json:"files"`
	Error         *string    `json:"error"`
	TimeRemaining *int64     `json:"timeRemaining"`
	Configured    bool       `json:"configured"`
	Selected      []int      `json:"selected"`
	Checking      bool       `json:"checking"`
	CheckedPieces int        `json:"checkedPieces"`
	CheckTotal    int        `json:"checkTotal"`
}

type PlayRequest struct {
	ID        string `json:"id"`
	FileIndex *int   `json:"fileIndex,omitempty"`
}

type PlayResponse struct {
	ID        string `json:"id"`
	FileIndex int    `json:"fileIndex"`
	Name      string `json:"name"`
	Length    int64  `json:"length"`
	StreamURL string `json:"streamUrl"`
}

type ConfigureRequest struct {
	ID          string `json:"id"`
	Descriptors []*int `json:"descriptors"`
	Selected    []int  `json:"selected"`
}

type SelectRequest struct {
	ID       string `json:"id"`
	Selected []int  `json:"selected"`
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
	Start     int `json:"start"`
	End       int `json:"end"`
	Total     int `json:"total"`
	Selected  int `json:"selected"`
	Verified  int `json:"verified"`
	Receiving int `json:"receiving"`
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
	MaxPeers int `json:"maxPeers"`
}

type SettingsRequest struct {
	MaxPeers int `json:"maxPeers"`
}

type OkResponse struct {
	Ok bool   `json:"ok"`
	ID string `json:"id,omitempty"`
}
