//go:build !unix

package main

import (
	"errors"
	"os"
)

func seekData(f *os.File, offset int64) (pos int64, ok bool, err error) {
	return 0, false, errors.New("sparse seeking is unavailable on this platform")
}

func punchHole(f *os.File, offset, length int64) (ok bool, err error) {
	return false, nil
}

func fileOccupiedBytes(f *os.File) (int64, error) {
	if f == nil {
		return 0, nil
	}
	stat, err := f.Stat()
	if err != nil {
		return 0, err
	}
	return stat.Size(), nil
}
