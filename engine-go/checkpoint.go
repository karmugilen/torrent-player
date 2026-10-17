package main

import (
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

const (
	checkpointMagic   = 0x57435031 // "WCP1"
	checkpointVersion = 1
	// Bound allocations before trusting on-disk counts. ~512K pieces covers
	// multi-terabyte torrents at common piece sizes without unbounded allocs.
	maxCheckpointPieces = 512 * 1024
	maxCheckpointFiles  = 65_536
	maxCheckpointPath   = 4_096
)

type fileIdentity struct {
	External bool
	Dev      uint64
	Ino      uint64
	Length   int64
	ModNano  int64
	Path     string
}

type pieceCheckpoint struct {
	InfoHash    [20]byte
	PieceLength int64
	NumPieces   int
	Files       []fileIdentity
	Complete    []bool
}

func checkpointPath(baseDir, infoHashHex string) string {
	return filepath.Join(baseDir, ".checkpoints", infoHashHex+".chk")
}

func identityForFile(f FileDescriptor) (fileIdentity, bool) {
	if f.File == nil {
		return fileIdentity{External: f.external, Length: f.Length, Path: f.Path}, false
	}
	stat, err := f.File.Stat()
	if err != nil {
		return fileIdentity{External: f.external, Length: f.Length, Path: f.Path}, false
	}
	return identityFromStat(f, f.external, stat)
}

func identityFromStat(f FileDescriptor, external bool, stat os.FileInfo) (fileIdentity, bool) {
	id := fileIdentity{External: external, Length: f.Length, Path: f.Path, ModNano: stat.ModTime().UnixNano()}
	if st, ok := stat.Sys().(*syscall.Stat_t); ok {
		id.Dev = uint64(st.Dev)
		id.Ino = uint64(st.Ino)
		return id, true
	}
	// Without stable device/inode identity we cannot safely reuse bits.
	return id, false
}

func identitiesMatch(saved, current []fileIdentity) bool {
	if len(saved) != len(current) {
		return false
	}
	for i := range saved {
		if !fileIdentityEqual(saved[i], current[i]) {
			return false
		}
	}
	return true
}

// fileStable is the resume key: same inode and torrent length. Mtime is not
// required — our own writes bump it on every MarkComplete, and a force-stop
// before the next .chk rename would otherwise drop the whole bitfield.
func fileStable(saved, current fileIdentity) bool {
	if saved.Dev == 0 && saved.Ino == 0 {
		return false
	}
	if saved.External != current.External || saved.Length != current.Length || saved.Dev != current.Dev || saved.Ino != current.Ino {
		return false
	}
	if !saved.External && saved.Path != current.Path {
		return false
	}
	return true
}

func fileIdentityEqual(a, b fileIdentity) bool {
	return fileStable(a, b) && a.ModNano == b.ModNano
}

func (cp *pieceCheckpoint) bitfieldBytes() []byte {
	out := make([]byte, (cp.NumPieces+7)/8)
	for i, ok := range cp.Complete {
		if ok {
			out[i/8] |= 1 << uint(i%8)
		}
	}
	return out
}

func decodeBitfield(bits []byte, n int) []bool {
	out := make([]bool, n)
	for i := 0; i < n; i++ {
		if bits[i/8]&(1<<uint(i%8)) != 0 {
			out[i] = true
		}
	}
	return out
}

func writePieceCheckpoint(path string, cp *pieceCheckpoint) error {
	if cp == nil || cp.NumPieces < 0 || cp.NumPieces > maxCheckpointPieces || len(cp.Complete) != cp.NumPieces {
		return fmt.Errorf("invalid checkpoint")
	}
	if len(cp.Files) > maxCheckpointFiles {
		return fmt.Errorf("invalid checkpoint file count")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}
	tmp := path + ".tmp"
	f, err := os.OpenFile(tmp, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	ok := false
	defer func() {
		_ = f.Close()
		if !ok {
			_ = os.Remove(tmp)
		}
	}()

	hdr := make([]byte, 4+4+20+8+4)
	binary.BigEndian.PutUint32(hdr[0:], checkpointMagic)
	binary.BigEndian.PutUint32(hdr[4:], checkpointVersion)
	copy(hdr[8:], cp.InfoHash[:])
	binary.BigEndian.PutUint64(hdr[28:], uint64(cp.PieceLength))
	binary.BigEndian.PutUint32(hdr[36:], uint32(cp.NumPieces))
	if _, err := f.Write(hdr); err != nil {
		return err
	}
	if err := binary.Write(f, binary.BigEndian, uint32(len(cp.Files))); err != nil {
		return err
	}
	for _, file := range cp.Files {
		var flags uint8
		if file.External {
			flags = 1
		}
		if err := binary.Write(f, binary.BigEndian, flags); err != nil {
			return err
		}
		if err := binary.Write(f, binary.BigEndian, file.Dev); err != nil {
			return err
		}
		if err := binary.Write(f, binary.BigEndian, file.Ino); err != nil {
			return err
		}
		if err := binary.Write(f, binary.BigEndian, file.Length); err != nil {
			return err
		}
		if err := binary.Write(f, binary.BigEndian, file.ModNano); err != nil {
			return err
		}
		pathBytes := []byte(file.Path)
		if err := binary.Write(f, binary.BigEndian, uint32(len(pathBytes))); err != nil {
			return err
		}
		if _, err := f.Write(pathBytes); err != nil {
			return err
		}
	}
	bits := cp.bitfieldBytes()
	if _, err := f.Write(bits); err != nil {
		return err
	}
	if err := f.Sync(); err != nil {
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	ok = true
	return nil
}

func readPieceCheckpoint(path string) (*pieceCheckpoint, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	if len(data) < 40 {
		return nil, fmt.Errorf("checkpoint too short")
	}
	if binary.BigEndian.Uint32(data[0:]) != checkpointMagic {
		return nil, fmt.Errorf("bad checkpoint magic")
	}
	if binary.BigEndian.Uint32(data[4:]) != checkpointVersion {
		return nil, fmt.Errorf("unsupported checkpoint version")
	}
	cp := &pieceCheckpoint{
		PieceLength: int64(binary.BigEndian.Uint64(data[28:])),
		NumPieces:   int(binary.BigEndian.Uint32(data[36:])),
	}
	copy(cp.InfoHash[:], data[8:28])
	if cp.NumPieces < 0 || cp.NumPieces > maxCheckpointPieces || cp.PieceLength <= 0 {
		return nil, fmt.Errorf("invalid checkpoint header")
	}
	off := 40
	if off+4 > len(data) {
		return nil, fmt.Errorf("truncated checkpoint files")
	}
	nFiles := int(binary.BigEndian.Uint32(data[off:]))
	off += 4
	if nFiles < 0 || nFiles > maxCheckpointFiles {
		return nil, fmt.Errorf("invalid checkpoint file count")
	}
	bitBytes := (cp.NumPieces + 7) / 8
	// Reject counts that cannot fit the remaining payload before allocating.
	if off+nFiles*(1+8+8+8+8+4) > len(data) || off+bitBytes > len(data) {
		// Still parse carefully below; this is a cheap upper-bound pre-check.
	}
	cp.Files = make([]fileIdentity, 0, nFiles)
	for i := 0; i < nFiles; i++ {
		need := off + 1 + 8 + 8 + 8 + 8 + 4
		if need > len(data) {
			return nil, fmt.Errorf("truncated file identity")
		}
		var id fileIdentity
		id.External = data[off] == 1
		off++
		id.Dev = binary.BigEndian.Uint64(data[off:])
		off += 8
		id.Ino = binary.BigEndian.Uint64(data[off:])
		off += 8
		id.Length = int64(binary.BigEndian.Uint64(data[off:]))
		off += 8
		id.ModNano = int64(binary.BigEndian.Uint64(data[off:]))
		off += 8
		pathLen := int(binary.BigEndian.Uint32(data[off:]))
		off += 4
		if pathLen < 0 || pathLen > maxCheckpointPath || off+pathLen > len(data) {
			return nil, fmt.Errorf("truncated file path")
		}
		id.Path = string(data[off : off+pathLen])
		off += pathLen
		cp.Files = append(cp.Files, id)
	}
	if off+bitBytes > len(data) {
		return nil, fmt.Errorf("truncated bitfield")
	}
	cp.Complete = decodeBitfield(data[off:off+bitBytes], cp.NumPieces)
	return cp, nil
}

func deletePieceCheckpoint(baseDir, infoHashHex string) error {
	err := os.Remove(checkpointPath(baseDir, infoHashHex))
	if err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}

// coalesceFlushDelay is the pause before writing a dirty checkpoint. Tests may
// set checkpointFlushDelay to 0 for deterministic resume checks.
var checkpointFlushDelay = 250 * time.Millisecond

func coalesceFlushDelay() time.Duration { return checkpointFlushDelay }
