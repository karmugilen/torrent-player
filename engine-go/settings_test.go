package main

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"testing"

	"github.com/anacrolix/torrent/metainfo"
)

func TestSettingsApplyToExistingAndFutureTorrents(t *testing.T) {
	s := testEngine(t)
	add := func(name string) *TorrentRecord {
		t.Helper()
		mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
		if err != nil {
			t.Fatal(err)
		}
		if name != "" {
			// A distinct torrent without introducing tracker/network dependencies.
			mi.InfoBytes = bytes.Replace(mi.InfoBytes, []byte("tiny"), []byte(name), 1)
		}
		var data bytes.Buffer
		if err := mi.Write(&data); err != nil {
			t.Fatal(err)
		}
		body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(data.Bytes())})
		code, response := s.DispatchControl("POST", "/add", string(body))
		if code != 200 {
			t.Fatalf("add: %d %s", code, response)
		}
		var added AddResponse
		if err := json.Unmarshal(response, &added); err != nil {
			t.Fatal(err)
		}
		return s.records[added.ID]
	}
	first := add("")
	code, body := s.DispatchControl("POST", "/settings", `{"maxPeers":500,"ramCacheBytes":536870912}`)
	if code != 200 {
		t.Fatalf("settings: %d %s", code, body)
	}
	var settings SettingsResponse
	if err := json.Unmarshal(body, &settings); err != nil {
		t.Fatal(err)
	}
	if settings.MaxPeers != 500 || settings.RamCacheBytes != 512<<20 {
		t.Fatalf("unexpected settings: %+v", settings)
	}
	second := add("next")
	if second == first {
		t.Fatal("expected two distinct torrents")
	}
	for _, rec := range []*TorrentRecord{first, second} {
		if old := rec.Torrent.SetMaxEstablishedConns(500); old != 500 {
			t.Fatalf("connection cap = %d, want 500", old)
		}
		st := s.storageForTorrent(rec.Torrent)
		st.mu.Lock()
		limit, stream := st.maxCacheBytes, st.streamMaxBytes
		st.mu.Unlock()
		if limit != 512<<20 || stream != limit {
			t.Fatalf("cache limits = %d/%d", limit, stream)
		}
	}
	code, body = s.DispatchControl("POST", "/settings", `{"maxPeers":55}`)
	if code != 200 || s.storage.CacheLimit() != 512<<20 {
		t.Fatalf("unrelated setting reset cache: %d %s", code, body)
	}
	for _, rec := range []*TorrentRecord{first, second} {
		if old := rec.Torrent.SetMaxEstablishedConns(55); old != 55 {
			t.Fatalf("lowered connection cap = %d, want 55", old)
		}
	}
	for _, invalid := range []string{
		`{"maxPeers":501}`, `{"maxPeers":7}`,
		`{"ramCacheBytes":16777215}`, `{"ramCacheBytes":1073741825}`,
	} {
		if code, body := s.DispatchControl("POST", "/settings", invalid); code != 400 {
			t.Fatalf("accepted %s: %d %s", invalid, code, body)
		}
	}
	for _, limit := range []int64{16 << 20, 64 << 20, 1024 << 20} {
		if code, body := s.DispatchControl("POST", "/settings", fmt.Sprintf(`{"ramCacheBytes":%d}`, limit)); code != 200 {
			t.Fatalf("valid cache boundary: %d %s", code, body)
		}
	}
}

func TestCacheShrinkPreservesDirtyDownloadBytes(t *testing.T) {
	info := &metainfo.Info{Name: "data", PieceLength: 4, Length: 12, Pieces: make([]byte, 60)}
	client := NewDocumentStorageClient(t.TempDir())
	if err := client.SetCacheLimit(8); err != nil {
		t.Fatal(err)
	}
	impl, err := client.OpenTorrent(context.Background(), info, metainfo.Hash{})
	if err != nil {
		t.Fatal(err)
	}
	defer impl.Close()
	st := client.GetStorage(metainfo.Hash{}.HexString())
	dest, err := os.CreateTemp(t.TempDir(), "download")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	for i, payload := range []string{"AAAA", "BBBB", "CCCC"} {
		if i == 2 {
			if err := client.SetCacheLimit(4); err != nil {
				t.Fatal(err)
			}
		}
		if _, err := impl.Piece(info.Piece(i)).WriteAt([]byte(payload), 0); err != nil {
			t.Fatal(err)
		}
	}
	got := make([]byte, 8)
	if _, err := dest.ReadAt(got, 0); err != nil || string(got) != "AAAABBBB" {
		t.Fatalf("eviction lost dirty data: %q, %v", got, err)
	}
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.cacheBytes > st.maxCacheBytes {
		t.Fatalf("cache exceeds new cap: %d/%d", st.cacheBytes, st.maxCacheBytes)
	}
}

func TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded(t *testing.T) {
	st, data := directStorageFixture(t, t.TempDir(), 1<<20, 12)
	putDirectPiece(t, st, data, 6)
	st.setCacheLimit(128 << 20)
	got := make([]byte, 32)
	if n, _, err := st.readVerifiedStream(got, 6<<20); err != nil || n != len(got) || !bytes.Equal(got, data[6<<20:(6<<20)+32]) {
		t.Fatalf("resize invalidated verified data: n=%d err=%v", n, err)
	}
	preview := newDocumentTorrentStorage(st.info, metainfo.Hash{}, t.TempDir())
	defer preview.Close()
	if err := preview.attachPreviewTemp(0); err != nil {
		t.Fatal(err)
	}
	preview.setCacheLimit(256 << 20)
	if preview.maxCacheBytes != previewBudgetBytes {
		t.Fatal("setting expanded preview task budget")
	}
	preview.releasePreviewTemp()
	if preview.maxCacheBytes != 256<<20 {
		t.Fatal("preview release did not restore configured download budget")
	}
}

// A small RAM setting must not refuse or break Direct Play: each torrent keeps
// at least four of its pieces, and the setting itself is kept as chosen.
func TestSmallRAMSettingKeepsFourDirectPlayPieces(t *testing.T) {
	info := &metainfo.Info{Name: "large-pieces", PieceLength: 32 << 20, Length: 256 << 20, Pieces: make([]byte, 160)}
	client := NewDocumentStorageClient(t.TempDir())
	if err := client.SetCacheLimit(16 << 20); err != nil {
		t.Fatal(err)
	}
	impl, err := client.OpenTorrent(context.Background(), info, metainfo.Hash{})
	if err != nil {
		t.Fatal(err)
	}
	defer impl.Close()
	st := client.GetStorage(metainfo.Hash{}.HexString())
	if err := st.AttachDescriptorsMode([]*int{nil}, []int{0}, true); err != nil {
		t.Fatalf("16 MiB setting refused 32 MiB pieces: %v", err)
	}
	if st.maxCacheBytes != 128<<20 {
		t.Fatalf("Direct Play cache %d, want four pieces", st.maxCacheBytes)
	}
	if err := client.SetCacheLimit(512 << 20); err != nil || st.maxCacheBytes != 512<<20 {
		t.Fatalf("larger setting: %v %d", err, st.maxCacheBytes)
	}
	if err := client.SetCacheLimit(16 << 20); err != nil {
		t.Fatalf("shrinking below four pieces was refused: %v", err)
	}
	if client.CacheLimit() != 16<<20 || st.maxCacheBytes != 128<<20 {
		t.Fatalf("setting %d, Direct Play cache %d", client.CacheLimit(), st.maxCacheBytes)
	}
}
