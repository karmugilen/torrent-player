//go:build !rust

package main

import (
	"crypto/sha1"
	"fmt"
	"io"
	"syscall"
)

func hashExtents(spans []fileSpan) ([20]byte, bool, error) {
	h := sha1.New()
	buf := make([]byte, 64*1024)
	allHoles := true
	for _, span := range spans {
		if span.fd < 0 || span.length < 0 || span.offset < 0 {
			return [20]byte{}, false, fmt.Errorf("invalid span")
		}
		if span.length == 0 {
			continue
		}
		holes, err := hashSpan(h, span, buf)
		if err != nil {
			return [20]byte{}, false, err
		}
		if !holes {
			allHoles = false
		}
	}
	var out [20]byte
	copy(out[:], h.Sum(nil))
	return out, allHoles, nil
}

func hashSpan(h io.Writer, span fileSpan, buf []byte) (allHoles bool, err error) {
	allHoles = true
	end := span.offset + span.length
	abs := span.offset
	for abs < end {
		dataPos, ok, seekErr := seekDataFd(int(span.fd), abs)
		if !ok {
			return hashSpanDense(h, span.fd, abs, end-abs, buf)
		}
		if seekErr == io.EOF {
			writeZeroHash(h, end-abs)
			return allHoles, nil
		}
		if seekErr != nil {
			return false, seekErr
		}
		if dataPos >= end {
			writeZeroHash(h, end-abs)
			return allHoles, nil
		}
		if dataPos > abs {
			writeZeroHash(h, dataPos-abs)
			abs = dataPos
		}
		holePos, holeOK, holeErr := seekHoleFd(int(span.fd), abs)
		if !holeOK || holeErr != nil {
			dense, err := hashSpanDense(h, span.fd, abs, end-abs, buf)
			if !dense {
				allHoles = false
			}
			return allHoles, err
		}
		if holePos > end {
			holePos = end
		}
		if holePos <= abs {
			dense, err := hashSpanDense(h, span.fd, abs, end-abs, buf)
			if !dense {
				allHoles = false
			}
			return allHoles, err
		}
		allHoles = false
		remaining := holePos - abs
		for remaining > 0 {
			n := int(remaining)
			if n > len(buf) {
				n = len(buf)
			}
			got, err := preadFd(int(span.fd), buf[:n], abs)
			if got > 0 {
				_, _ = h.Write(buf[:got])
				abs += int64(got)
				remaining -= int64(got)
			}
			if err != nil {
				return false, err
			}
			if got == 0 {
				return false, io.ErrUnexpectedEOF
			}
		}
	}
	return allHoles, nil
}

func hashSpanDense(h io.Writer, fd int32, offset, length int64, buf []byte) (allHoles bool, err error) {
	allHoles = true
	var pos int64
	for pos < length {
		n := int(length - pos)
		if n > len(buf) {
			n = len(buf)
		}
		got, err := preadFd(int(fd), buf[:n], offset+pos)
		if got > 0 {
			for _, b := range buf[:got] {
				if b != 0 {
					allHoles = false
					break
				}
			}
			_, _ = h.Write(buf[:got])
			pos += int64(got)
		}
		if err != nil {
			return false, err
		}
		if got == 0 {
			return false, io.ErrUnexpectedEOF
		}
	}
	return allHoles, nil
}

func writeZeroHash(h io.Writer, n int64) {
	var zeros [32 * 1024]byte
	for n > 0 {
		chunk := int64(len(zeros))
		if chunk > n {
			chunk = n
		}
		_, _ = h.Write(zeros[:chunk])
		n -= chunk
	}
}

func preadExtents(spans []fileSpan, buf []byte) (int, error) {
	done := 0
	for _, span := range spans {
		if span.fd < 0 || span.length < 0 || span.offset < 0 {
			return done, fmt.Errorf("invalid span")
		}
		need := int(span.length)
		if done+need > len(buf) {
			return done, fmt.Errorf("buffer too small")
		}
		got := 0
		for got < need {
			n, err := preadFd(int(span.fd), buf[done+got:done+need], span.offset+int64(got))
			if n > 0 {
				got += n
			}
			if err != nil {
				return done + got, err
			}
			if n == 0 {
				return done + got, io.ErrUnexpectedEOF
			}
		}
		done += need
	}
	return done, nil
}

func preadFd(fd int, buf []byte, offset int64) (int, error) {
	n, err := syscall.Pread(fd, buf, offset)
	if n < 0 {
		n = 0
	}
	return n, err
}
