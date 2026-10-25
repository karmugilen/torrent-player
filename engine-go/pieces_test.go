package main

import (
	"math"
	"reflect"
	"testing"

	"github.com/anacrolix/torrent"
)

func pieceStateRun(length int, complete, partial, checking, hashing bool) torrent.PieceStateRun {
	var state torrent.PieceState
	state.Complete = complete
	state.Partial = partial
	state.Checking = checking
	state.Hashing = hashing
	return torrent.PieceStateRun{PieceState: state, Length: length}
}

func TestCompressPieceRunsCountsOnlyWantedPartialsWhilePlaying(t *testing.T) {
	left := pieceStateRun(2, false, true, false, false)
	left.Priority = torrent.PiecePriorityNormal
	wanted := pieceStateRun(3, false, true, false, false)
	wanted.Priority = torrent.PiecePriorityNext
	got, err := compressPieceRuns([]torrent.PieceStateRun{left, wanted}, 5, true)
	if err != nil || !reflect.DeepEqual(got, []pieceRun{{length: 2, flags: 0}, {length: 3, flags: 2}}) {
		t.Fatalf("playing: %+v %v", got, err)
	}
	got, err = compressPieceRuns([]torrent.PieceStateRun{left, wanted}, 5, false)
	if err != nil || !reflect.DeepEqual(got, []pieceRun{{length: 5, flags: 2}}) {
		t.Fatalf("not playing: %+v %v", got, err)
	}
}

func TestCompressPieceRuns(t *testing.T) {
	high := pieceStateRun(4, true, false, false, false)
	high.Priority = torrent.PiecePriorityNow
	high.Ok = true
	low := pieceStateRun(6, true, false, false, false)
	low.Priority = torrent.PiecePriorityNone
	low.QueuedForHash = true
	low.Marking = true
	low.Ok = false

	states := []torrent.PieceStateRun{
		high,
		low,
		pieceStateRun(2, true, true, false, false),
		pieceStateRun(4, false, true, false, false),
		pieceStateRun(1, false, false, true, false),
		pieceStateRun(6, false, false, false, true),
		pieceStateRun(7, false, false, false, false),
	}
	got, err := compressPieceRuns(states, 4+6+2+4+1+6+7, false)
	if err != nil {
		t.Fatal(err)
	}
	// Complete+Partial collapses to verified (1) and merges with the prior verified span.
	want := []pieceRun{
		{length: 12, flags: 1},
		{length: 11, flags: 2},
		{length: 7, flags: 0},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("compressed runs:\n got %+v\nwant %+v", got, want)
	}

	onlyChecking, err := compressPieceRuns([]torrent.PieceStateRun{
		pieceStateRun(3, false, false, true, false),
	}, 3, false)
	if err != nil || !reflect.DeepEqual(onlyChecking, []pieceRun{{length: 3, flags: 2}}) {
		t.Fatalf("checking run: %+v %v", onlyChecking, err)
	}
	onlyHashing, err := compressPieceRuns([]torrent.PieceStateRun{
		pieceStateRun(3, false, false, false, true),
	}, 3, false)
	if err != nil || !reflect.DeepEqual(onlyHashing, []pieceRun{{length: 3, flags: 2}}) {
		t.Fatalf("hashing run: %+v %v", onlyHashing, err)
	}

	same := make([]torrent.PieceStateRun, 200)
	for i := range same {
		same[i] = pieceStateRun(1, false, true, false, false)
	}
	merged, err := compressPieceRuns(same, len(same), false)
	if err != nil {
		t.Fatal(err)
	}
	if len(merged) != 1 || merged[0] != (pieceRun{length: 200, flags: 2}) {
		t.Fatalf("expected one merged run, got %+v", merged)
	}
	if cap(merged) > 64 {
		t.Fatalf("cap %d sized from the anacrolix run list", cap(merged))
	}

	empty, err := compressPieceRuns(nil, 0, false)
	if err != nil || len(empty) != 0 {
		t.Fatalf("empty: %+v %v", empty, err)
	}
	short, err := compressPieceRuns([]torrent.PieceStateRun{pieceStateRun(2, true, false, false, false)}, 5, false)
	if err == nil || short != nil {
		t.Fatalf("short coverage must fail closed: %+v %v", short, err)
	}
}

func TestCompressPieceRunsRejectsBadLengths(t *testing.T) {
	valid := pieceStateRun(3, true, false, false, false)
	cases := [][]torrent.PieceStateRun{
		{pieceStateRun(0, true, false, false, false)},
		{pieceStateRun(-1, false, true, false, false)},
		{valid, pieceStateRun(0, false, false, false, false)},
		{pieceStateRun(3, false, false, false, false), pieceStateRun(3, false, true, false, false)},
		nil,
	}
	totals := []int{3, 3, 3, 5, -1}
	for i, states := range cases {
		got, err := compressPieceRuns(states, totals[i], false)
		if err == nil || got != nil {
			t.Fatalf("case %d: got %+v err %v", i, got, err)
		}
	}
}

func TestCompressPieceRunsUint32Limit(t *testing.T) {
	if uint64(math.MaxInt) <= uint64(math.MaxUint32) {
		t.Skip("int cannot represent lengths above MaxUint32")
	}
	limit := uint64(math.MaxUint32)
	overflow := int(limit + 1)
	got, err := compressPieceRuns([]torrent.PieceStateRun{{Length: overflow}}, overflow+10, false)
	if err == nil || got != nil {
		t.Fatalf("length above MaxUint32 truncated to %+v err %v", got, err)
	}
	got, err = compressPieceRuns([]torrent.PieceStateRun{{Length: int(limit)}, {Length: 1}}, int(limit)+10, false)
	if err == nil || got != nil {
		t.Fatalf("sum above MaxUint32: %+v %v", got, err)
	}
	exact := pieceStateRun(0, true, false, false, false)
	exact.Length = int(limit)
	got, err = compressPieceRuns([]torrent.PieceStateRun{exact}, int(limit), false)
	if err != nil || !reflect.DeepEqual(got, []pieceRun{{length: math.MaxUint32, flags: 1}}) {
		t.Fatalf("exact MaxUint32: %+v %v", got, err)
	}
}

func TestAggregatePieceBuckets(t *testing.T) {
	t.Run("overlap precedence and geometry", func(t *testing.T) {
		runs := []pieceRun{{length: 3, flags: 3}, {length: 3, flags: 2}, {length: 4, flags: 0}}
		ranges := []pieceRange{{start: 4, end: 8}, {start: 1, end: 5}, {start: 1, end: 5}}
		got, err := aggregatePieceBuckets(runs, ranges, 10, 4)
		if err != nil {
			t.Fatal(err)
		}
		want := []PieceBucket{
			{Start: 0, End: 2, Total: 3, Selected: 2, Verified: 3, SelectedVerified: 2},
			{Start: 3, End: 5, Total: 3, Selected: 3, Receiving: 3, SelectedReceiving: 3},
			{Start: 6, End: 8, Total: 3, Selected: 2},
			{Start: 9, End: 9, Total: 1},
		}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("buckets:\n got %+v\nwant %+v", got, want)
		}
	})

	t.Run("shared boundary counted once", func(t *testing.T) {
		runs := []pieceRun{{length: 4, flags: 1}, {length: 2, flags: 2}}
		// [0, 3) and [2, 5) share piece 2; [3, 3) is empty. Piece 5 stays outside.
		ranges := []pieceRange{{start: 2, end: 5}, {start: 0, end: 3}, {start: 3, end: 3}}
		got, err := aggregatePieceBuckets(runs, ranges, 6, 1)
		if err != nil {
			t.Fatal(err)
		}
		want := []PieceBucket{{
			Start: 0, End: 5, Total: 6,
			Selected: 5, Verified: 4, Receiving: 2,
			SelectedVerified: 4, SelectedReceiving: 1,
		}}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("buckets:\n got %+v\nwant %+v", got, want)
		}
	})

	t.Run("verified wins across a bucket boundary", func(t *testing.T) {
		got, err := aggregatePieceBuckets([]pieceRun{{length: 5, flags: 3}}, nil, 5, 2)
		if err != nil {
			t.Fatal(err)
		}
		want := []PieceBucket{
			{Start: 0, End: 2, Total: 3, Verified: 3},
			{Start: 3, End: 4, Total: 2, Verified: 2},
		}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("buckets:\n got %+v\nwant %+v", got, want)
		}
	})

	t.Run("merged runs match split runs", func(t *testing.T) {
		ranges := []pieceRange{{start: 1, end: 6}}
		split, err := aggregatePieceBuckets([]pieceRun{{length: 2, flags: 1}, {length: 2, flags: 1}, {length: 3, flags: 2}}, append([]pieceRange(nil), ranges...), 7, 3)
		if err != nil {
			t.Fatal(err)
		}
		merged, err := aggregatePieceBuckets([]pieceRun{{length: 4, flags: 1}, {length: 3, flags: 2}}, ranges, 7, 3)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(split, merged) {
			t.Fatalf("split %+v merged %+v", split, merged)
		}
	})

	t.Run("empty", func(t *testing.T) {
		got, err := aggregatePieceBuckets(nil, nil, 0, 256)
		if err != nil || len(got) != 0 {
			t.Fatalf("empty: %+v %v", got, err)
		}
	})

	t.Run("bounds", func(t *testing.T) {
		if _, err := aggregatePieceBuckets(nil, nil, 0, 0); err == nil {
			t.Fatal("expected maxBuckets < 1 to fail")
		}
		if _, err := aggregatePieceBuckets(nil, nil, 1, 1025); err == nil {
			t.Fatal("expected maxBuckets > 1024 to fail")
		}
	})

	t.Run("large total few runs", func(t *testing.T) {
		const total = 200_000
		const maxBuckets = 256
		runs := []pieceRun{{length: 1000, flags: 1}, {length: 500, flags: 2}, {length: total - 1500, flags: 0}}
		ranges := []pieceRange{{start: 1100, end: 1800}, {start: 100, end: 1200}}
		got, err := aggregatePieceBuckets(runs, ranges, total, maxBuckets)
		if err != nil {
			t.Fatal(err)
		}
		const size = (total + maxBuckets - 1) / maxBuckets
		if len(got) != (total+size-1)/size {
			t.Fatalf("bucket count %d", len(got))
		}
		var sum PieceBucket
		for i, b := range got {
			start := i * size
			end := min(start+size, total) - 1
			if b.Start != start || b.End != end || b.Total != end-start+1 {
				t.Fatalf("bucket %d geometry %+v", i, b)
			}
			sum.Total += b.Total
			sum.Selected += b.Selected
			sum.Verified += b.Verified
			sum.Receiving += b.Receiving
			sum.SelectedVerified += b.SelectedVerified
			sum.SelectedReceiving += b.SelectedReceiving
		}
		if sum.Total != total || sum.Verified != 1000 || sum.Receiving != 500 || sum.Selected != 1700 || sum.SelectedVerified != 900 || sum.SelectedReceiving != 500 {
			t.Fatalf("sums %+v", sum)
		}
		if !reflect.DeepEqual(got[0], PieceBucket{Start: 0, End: 781, Total: 782, Selected: 682, Verified: 782, SelectedVerified: 682}) {
			t.Fatalf("bucket 0 %+v", got[0])
		}
		if !reflect.DeepEqual(got[1], PieceBucket{Start: 782, End: 1563, Total: 782, Selected: 782, Verified: 218, Receiving: 500, SelectedVerified: 218, SelectedReceiving: 500}) {
			t.Fatalf("bucket 1 %+v", got[1])
		}
		if !reflect.DeepEqual(got[2], PieceBucket{Start: 1564, End: 2345, Total: 782, Selected: 236}) {
			t.Fatalf("bucket 2 %+v", got[2])
		}
		last := got[len(got)-1]
		if last.Start != 199410 || last.End != 199999 || last.Total != 590 || last.Selected != 0 || last.Verified != 0 || last.Receiving != 0 {
			t.Fatalf("last bucket %+v", last)
		}
	})
}
