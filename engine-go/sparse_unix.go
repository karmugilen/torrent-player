//go:build unix

package main

import (
	"io"
	"os"
	"syscall"

	"golang.org/x/sys/unix"
)

// seekData finds the next data offset at or after offset.
// ok=false means the filesystem does not expose sparse extents.
// io.EOF means there is no more data at or after offset (remaining bytes are holes).
func seekData(f *os.File, offset int64) (pos int64, ok bool, err error) {
	pos, err = unix.Seek(int(f.Fd()), offset, unix.SEEK_DATA)
	if err == nil {
		return pos, true, nil
	}
	if err == unix.ENXIO {
		return 0, true, io.EOF
	}
	// EINVAL/ENOTSUP and similar: provider or filesystem lacks hole seeking.
	return 0, false, err
}

// punchHole discards file data in [offset, offset+length) while keeping the
// logical size. ok=false means the filesystem does not support hole punching.
func punchHole(f *os.File, offset, length int64) (ok bool, err error) {
	if f == nil || length <= 0 {
		return true, nil
	}
	err = unix.Fallocate(int(f.Fd()), unix.FALLOC_FL_PUNCH_HOLE|unix.FALLOC_FL_KEEP_SIZE, offset, length)
	if err == nil {
		return true, nil
	}
	if err == unix.EOPNOTSUPP || err == unix.ENOTSUP || err == unix.EINVAL {
		return false, nil
	}
	return false, err
}

func fileOccupiedBytes(f *os.File) (int64, error) {
	if f == nil {
		return 0, nil
	}
	stat, err := f.Stat()
	if err != nil {
		return 0, err
	}
	if st, ok := stat.Sys().(*syscall.Stat_t); ok {
		return int64(st.Blocks) * 512, nil
	}
	return stat.Size(), nil
}
