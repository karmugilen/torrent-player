package main

import "time"

type nameRateBucket struct {
	second                  int64
	live, indexed, verified int
	lookups                 int
}

// Count actual newly named hashes over the last 60 seconds. Repeated catalog
// rows and names loaded from disk do not inflate the live resolver rate.
func (d *dashboard) recordNameRateLocked(source string, observed bool, now time.Time) {
	second := now.Unix()
	bucket := &d.nameRates[second%60]
	if bucket.second != second {
		*bucket = nameRateBucket{second: second}
	}
	if source == "cache" || source == "peer" {
		bucket.verified++
		bucket.live++
	} else if observed {
		bucket.live++
	} else {
		bucket.indexed++
	}
}

func (d *dashboard) applyNameRatesLocked(s *state, now time.Time) {
	for _, bucket := range d.nameRates {
		if age := now.Unix() - bucket.second; age >= 0 && age < 60 {
			s.LiveNamesPerMinute += bucket.live
			s.IndexNamesPerMinute += bucket.indexed
			s.VerifiedNamesPerMinute += bucket.verified
			s.LookupsPerMinute += bucket.lookups
		}
	}
}

func (d *dashboard) recordLookupRateLocked(now time.Time) {
	second := now.Unix()
	bucket := &d.nameRates[second%60]
	if bucket.second != second {
		*bucket = nameRateBucket{second: second}
	}
	bucket.lookups++
}
