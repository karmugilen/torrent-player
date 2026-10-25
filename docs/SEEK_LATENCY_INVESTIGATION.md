# Seek latency investigation — 2026-10-25

The remaining seek cost in the measured engine path is mostly waiting for peer
data and completing the player's read buffer. The engine already schedules the
new target quickly. Neither stricter peer selection nor earlier request stealing
showed a consistent improvement worth changing the runtime defaults for.

## What was measured

Tests ran on Linux x86_64 with Go 1.27.2 against the current working tree,
including its existing playback and torrent-library changes. No Android device
was connected (`adb devices` was empty).

The existing local swarm supplies 16 TCP seed candidates, with upload limits of
40, 150, 400 and 1,024 KiB/s, 256 KiB pieces, and a 192 MiB generated payload.
It waits for at least 11 connected seeds. It injects 20–199 ms delay into the
leecher's incoming TCP reads; writes are not delayed. Although older harness
comments describe 40–400 ms round trips, the wrapper does not inject both legs.
These tests do not model packet loss, mobile radios, DHT discovery, Android
callback dispatch, container parsing or decoding.

Seeks choose missing pieces and pause 300 ms between reads. Variants are
interleaved on the same swarm, with different random cold offsets. The timing
results are samples, not deterministic latency guarantees.

## Where the time goes

A 12-seek block trace at a 16 MiB cache gave these median stages, rounded to ms:

| Stage | Median |
| --- | ---: |
| Seek to request for the block at the requested offset | 1 ms |
| That request to that block arriving | 94 ms |
| That block arriving to the full 64 KiB read returning | 55 ms |
| Entire 64 KiB read | 144 ms |

The total p90 was 177 ms and the maximum was 292 ms. Individual stage medians
need not sum to the median total. A representative seek received the playhead
block at 95 ms but returned at 292 ms because another required block was on a
150 KiB/s seed. This distinguishes waiting for the rest of the buffer from slow
Go request scheduling.

## Controlled comparisons

The checked run used 30 timed seeks per variant and cache size, plus untimed
warmup reads. `partial` changes only the read API to `ReadSome`; `rescue100`
changes only `FocusStealDivisor` from 2 to 5, reducing the focused-block overdue
threshold from 250 to 100 ms.

| Cache | Variant | Median | p90 | Useful download | Duplicate data |
| --- | --- | ---: | ---: | ---: | ---: |
| 16 MiB | Current full read | 143 ms | 359 ms | 3.30 MiB/s | 0.1% |
| 16 MiB | Partial read | 192 ms | 328 ms | 3.27 MiB/s | 0.0% |
| 16 MiB | Rescue at 100 ms | 143 ms | 333 ms | 3.34 MiB/s | 0.4% |
| 128 MiB | Current full read | 126 ms | 294 ms | 4.13 MiB/s | 0.0% |
| 128 MiB | Partial read | 99 ms | 206 ms | 4.17 MiB/s | 0.0% |
| 128 MiB | Rescue at 100 ms | 130 ms | 297 ms | 4.16 MiB/s | 0.2% |

A full read returns all requested 64 KiB. A partial read returns the first
available contiguous bytes, which may be less than 16 KiB at an unaligned
offset. These are different delivery milestones. The HTTP-style result measures
the engine read API, not an actual HTTP request or decoded frame. Useful download
includes background buffering during the timed group and its 300 ms pauses.
Duplicate data is wasted chunks divided by useful bytes, rounded to 0.1%.

In the preceding exploratory run (25 timed seeks per variant), setting
`ArrivalRatio=1` instead of 1.5 changed median/p90 from 144/317 to 143/297 ms at
16 MiB, and 109/309 to 111/321 ms at 128 MiB. It did not consistently improve
latency. Partial reads measured 96/270 and 98/302 ms in that run. The differing
16 MiB results motivated a separate repeat rather than a claim of a uniform
speedup.

That separate, checked 16 MiB repeat used 50 timed seeks per variant:

| Variant | Median | p90 | Useful download | Duplicate data |
| --- | ---: | ---: | ---: | ---: |
| Current full read | 142 ms | 289 ms | 3.26 MiB/s | 0.0% |
| Partial read | 92 ms | 204 ms | 3.40 MiB/s | 0.0% |

Here partial delivery improved the median by 50 ms (35%) and p90 by 85 ms
(29%). It is the clearest delivery improvement to evaluate on a phone, with
the variability above retained as a limit on the claim.

## Practical priorities

1. **Compare Standard and HTTP with the actual player.** HTTP already uses
   `ReadSome`, so it can hand over the first available block. At 128 MiB this
   run improved median engine delivery by 27 ms (21%) and p90 by 88 ms (30%).
   Player buffering and decoding can absorb that saving, and the small-cache
   result was variable. Keep the player, torrent, cache and seek sequence fixed
   and measure seek-to-picture as well as the engine statistic.
2. **For Standard, investigate scheduling the entire first read as a group.**
   `SetRequestFocus` currently supplies an offset, and peer choice estimates
   arrival of an individual block. The first 32–128 KiB can still span peers
   with different delivery times. A candidate scheduler should minimize the
   completion time of the whole initial read while preserving sequential
   throughput. This is a proposed next experiment, not a measured improvement.
3. **Investigate cancellation separately for multi-second seeks.** Standard
   gives each descriptor one handler thread. A missing-data callback can occupy
   it before the next read offset reaches Go. The engine can cancel an old read
   when a newer read arrives, but cannot act on an offset Android has not yet
   dispatched. HTTP requests have a separate disconnect cancellation path.
4. **Keep the present stealing defaults.** Earlier rescue increased duplicate
   traffic without consistently reducing latency. Tightening the arrival ratio
   also did not establish a gain. A larger cache did not consistently make cold
   seeks slower in these runs, so reducing RAM is not a supported universal fix.

Android's [proxy read contract](https://developer.android.com/reference/android/os/ProxyFileDescriptorCallback#onRead(long,int,byte%5B%5D))
requires the requested byte count except at EOF. Standard therefore cannot just
switch to partial reads. The [StorageManager API](https://developer.android.com/reference/android/os/storage/StorageManager#openProxyFileDescriptor(int,android.os.ProxyFileDescriptorCallback,android.os.Handler))
confirms that callbacks execute on the supplied handler.

## Measurement and validation changes

`engine-go/seek_search_test.go` previously discarded read errors and byte counts,
and treated a 10-second timeout as a sample while leaving its goroutine alive.
The harness now fails on read errors, empty reads, incomplete full reads and
timeouts. A timeout closes the reader and joins its read before failing. This
changes test instrumentation only; no runtime defaults or APK were changed.
The harness comments now describe the injected inbound delay accurately.

The read-path regression checks passed: delivering body blocks before full-piece
verification, immediate HTTP partial delivery, and displacing older seek readers.
The deterministic HTTP test also checks that a full read remains blocked while
the rest of its buffer is missing.
All five targeted tests passed with the race detector, including cancellation
on a newer seek and on close (`ok webtor/engine-go`, 1.632 seconds):

```sh
go test -race -run '^(TestHTTPReadHandsOverTheFirstBlockAtOnce|TestSeekServesDeliveredBlocksBeforeThePieceIsVerified|TestOnlyTheLatestSeekPositionIsFetched|TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped|TestDirectPlayCancelsWaitOnSeekAndClose)$' -v -count=1 -timeout 60s .
```

The UI statistic starts when Go recognizes a backward jump or a forward jump of
at least 2 MiB after opening, and finishes when that read returns data. Its median
covers the most recent 20 recorded seeks. It excludes time before the callback
reaches Go and time spent decoding and displaying a frame.

## Reproduce

Run from `engine-go/`:

```sh
SEEK_SEARCH=trace go test -run '^TestSeekTrace$' -v -count=1 -timeout 90s .

SEEK_SEARCH=tune SEEK_ROUNDS=6 SEEK_CACHE=16,128 \
  SEEK_TUNE='baseline:;partial:partial=1;rescue100:FocusStealDivisor=5' \
  go test -run '^TestSeekTuningComparison$' -v -count=1 -timeout 4m .

SEEK_SEARCH=tune SEEK_ROUNDS=5 SEEK_CACHE=16,128 \
  SEEK_TUNE='baseline:;strict:ArrivalRatio=1;partial:partial=1' \
  go test -run '^TestSeekTuningComparison$' -v -count=1 -timeout 3m .

SEEK_SEARCH=tune SEEK_ROUNDS=10 SEEK_CACHE=16 \
  SEEK_TUNE='baseline:;partial:partial=1' \
  go test -run '^TestSeekTuningComparison$' -v -count=1 -timeout 3m .
```
