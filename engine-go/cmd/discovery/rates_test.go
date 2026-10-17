package main

import (
	"strings"
	"testing"
	"time"
)

func TestRollingNameRatesSeparateBulkImportsAndLiveResolution(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	live := strings.Repeat("a", 40)
	d.upsertSampleLocked(live, now)
	d.applyNameLocked(namePatch{Hash: live, Name: "Live", Source: "dump"}, now)
	d.applyNameLocked(namePatch{Hash: strings.Repeat("b", 40), Name: "Imported", Source: "dump"}, now)
	d.applyNameLocked(namePatch{Hash: strings.Repeat("c", 40), Name: "Peer", Source: "peer"}, now)
	d.applyNameLocked(namePatch{Hash: live, Name: "Duplicate", Source: "catalog"}, now)
	d.recordLookupRateLocked(now)
	d.recordLookupRateLocked(now)
	var s state
	d.applyNameRatesLocked(&s, now)
	if s.LookupsPerMinute != 2 || s.LiveNamesPerMinute != 2 || s.IndexNamesPerMinute != 1 || s.VerifiedNamesPerMinute != 1 {
		t.Fatalf("mixed name rates: %+v", s)
	}
	s = state{}
	d.applyNameRatesLocked(&s, now.Add(time.Minute))
	if s.LookupsPerMinute+s.LiveNamesPerMinute+s.IndexNamesPerMinute != 0 {
		t.Fatal("old burst remained in rolling window")
	}
}
