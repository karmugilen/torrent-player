package main

import (
	"slices"
	"testing"
	"time"
)

func TestPieceLRUOrdersByUseAndRemoves(t *testing.T) {
	var l pieceLRU
	for _, i := range []int{3, 1, 2} {
		l.touch(i)
	}
	l.touch(3) // 3 becomes the newest
	var order []int
	l.each(func(i int) bool { order = append(order, i); return true })
	if !slices.Equal(order, []int{1, 2, 3}) {
		t.Fatalf("order %v", order)
	}
	if oldest, ok := l.oldest(1); !ok || oldest != 2 {
		t.Fatalf("oldest skipping 1 = %d %v", oldest, ok)
	}
	l.remove(2)
	l.remove(42)
	if l.len() != 2 {
		t.Fatalf("len %d", l.len())
	}
	if _, ok := l.oldest(1); !ok {
		t.Fatal("3 should remain")
	}
	l.reset()
	if _, ok := l.oldest(-1); ok || l.len() != 0 {
		t.Fatal("reset left pieces")
	}
}

// Touching a piece used to scan the whole cache, so every block and every read
// slowed down as the RAM setting grew. Now a touch must cost the same with
// 64 or 65536 pieces.
func TestPieceLRUTouchDoesNotGrowWithTheCache(t *testing.T) {
	cost := func(pieces int) time.Duration {
		var l pieceLRU
		for i := 0; i < pieces; i++ {
			l.touch(i)
		}
		start := time.Now()
		for n := 0; n < 200000; n++ {
			l.touch(n % 8) // the pieces being played, oldest first
		}
		return time.Since(start)
	}
	small, large := cost(64), cost(65536)
	if large > 5*small+20*time.Millisecond {
		t.Fatalf("touch cost grew with the cache: 64 pieces %v, 65536 pieces %v", small, large)
	}
}

func TestMemoryLimitCoversEveryOpenCache(t *testing.T) {
	c := NewDocumentStorageClient(t.TempDir())
	if got := c.memoryLimitLocked(); got != memoryLimitOverhead {
		t.Fatalf("no torrents: %d", got)
	}
	c.torrents["a"], c.torrents["b"] = &DocumentTorrentStorage{}, &DocumentTorrentStorage{}
	if got := c.memoryLimitLocked(); got != 2*DefaultMaxCacheBytes+memoryLimitOverhead {
		t.Fatalf("default caches: %d", got)
	}
	c.cacheLimitBytes = 1 << 30
	if got := c.memoryLimitLocked(); got != 2<<30+memoryLimitOverhead {
		t.Fatalf("1 GiB caches: %d", got)
	}
}
