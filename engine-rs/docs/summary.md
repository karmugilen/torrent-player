## In plain words (updated 28 Oct 10:05)

**Where we are:** the Rust engine runs on your phone. It starts, adds magnets,
downloads and streams. All 8 parts are written. The torrent core and the
engine work end to end, and the control API answers exactly like Go
(30/30 match).

**Fixed so far (highlights):**
- Magnets found no peers: DHT took the uTP port, which switched off all
  discovery (8d2f401)
- The extra public trackers Go adds after metadata, and DHT searches that
  run to completion (9ca6b7e)
- uTP handshake with real clients (qBittorrent, libtorrent), TCP/uTP dial
  race, faster dial retries, and peers no longer dropped for harmless errors
- Speed: seek tuning applied, deeper request pipeline, TCP_NODELAY, and
  Go-style dropping of choking peers
- Engine features: tests waiting on features went from 70 to 38 (170 tests
  pass, 0 fail)

**Real-swarm results (same computer, Go vs Rust):**

| | Go | Rust |
|---|---|---|
| Sintel metadata | about 20 s | **1.5 s** |
| Sintel speed | about 0.6 MiB/s | **20-37 MiB/s** |
| Ubuntu metadata | 4-14 s | **3-5 s** |
| Ubuntu peers | 28-41 | 15-21 (behind) |
| Ubuntu speed | 3-12 MiB/s | 1.2-2 MiB/s (behind) |

**What is left:**
1. Ubuntu-style big swarms: peers and speed still 2.5-6x behind Go
   (grok-swarm, grok-wire)
2. 38 engine tests still waiting on features or faithful ports (codex-engine)
3. The full Go-vs-Rust benchmark, then merge into your main branch

**Latest APK:** artifacts/webtor-rust-engine-release-39572f2.apk (all fixes up
to 39572f2).

**Agents now:** grok-swarm and grok-wire (Ubuntu peers/speed), codex-engine
(engine features). Codex has 18 % of its weekly limit left, resetting on 2 Nov.

**Safety:** builds stay out of `/tmp`, at least 25 GB of disk is kept free, and
a governor pauses builds when free RAM drops below 10 %.
