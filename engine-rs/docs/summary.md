## In plain words (updated 18:05)

**Where we are:** 7 of the 8 parts are finished and tested, including the
torrent core. Two full Rust engines now download a torrent from each other
through the control API. The last part, the engine (`wt-engine`), still needs
about 95 smaller features that the Go tests check (preview, playback windows,
Ghost mode, trackers, telemetry).

**What works today:** two Rust clients can download a file from each other
over TCP and uTP and verify every byte. Magnet links work, and a seek gets its
first 64 KiB in 76 ms. The Android `libengine.so` builds. All 30 control-API
answers now match the Go engine (0 differences). The slow Direct Play save now
runs off the storage lock, like Go.

**What is left (3 of 13 milestones):**

1. The engine features the Go tests check. All 143 tests are written in Rust
   and 51 pass. 92 wait on features that agy-engine and agy-core are building.
   63 of the ports were too thin compared with Go (6 of them "passed" without
   really testing), so agy-tests is redoing those properly.
2. Speed/RAM/CPU comparison of the full engine against Go (agy-perf is
   running it now). The Rust core alone downloads at 43 MiB/s on loopback,
   against the Go engine's 19.5.
3. The final release (optimized) APK. A debug APK with the Rust engine is
   already installed on your phone, and the engine starts on it.

**Who is doing it:** 4 Antigravity agents, each on its own part, working on
their own. The machine rebooted at 16:05, which closed the earlier Codex
agents. Their work was saved and committed before the new agents started.

| Agent | Doing |
|---|---|
| agy-core | core done; now the playback and preview features the tests need |
| agy-engine | two-engine download done; now the server-side features the tests need |
| agy-tests | redoing the 63 thin test ports faithfully; switches tests on as features land |
| agy-perf | full-engine speed/RAM/CPU comparison vs Go |

**Safety:** builds stay out of `/tmp`, at least 25 GB of disk is kept free, and
a governor pauses builds when free RAM drops below 10 %.
