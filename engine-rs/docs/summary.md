## In plain words (updated 17:20)

**Where we are:** all the Rust code is written and it all compiles. Six of the
8 parts are finished and tested. The last two, the torrent core (`wt-core`) and
the engine/control API (`wt-engine`), work for basic downloads but still have
bugs in some cases.

**What works today:** two Rust clients can download a file from each other
over TCP and uTP and verify every byte. Magnet links work, and a seek gets its
first 64 KiB in 76 ms. The Android `libengine.so` builds. All 30 control-API
answers now match the Go engine (0 differences). The slow Direct Play save now
runs off the storage lock, like Go.

**What is left (4 of 13 milestones):**

1. Two full engines downloading from each other through the control API. The
   core bug behind the 0 % download is fixed, and the engine test is being re-run.
2. Port the 128 remaining Go engine tests for real (14 pass)
3. Speed/RAM/CPU comparison against Go: Rust must match or beat it. In a release
   build the Rust core downloads at 43 MiB/s on loopback, about 2.2x the Go
   engine's 19.5. The full engine comparison is still to run.
4. The final release (optimized) APK. A debug APK with the Rust engine is
   already installed on your phone, and the engine starts on it.

**Who is doing it:** 4 Antigravity agents, each on its own part, working on
their own. The machine rebooted at 16:05, which closed the earlier Codex
agents. Their work was saved and committed before the new agents started.

| Agent | Doing |
|---|---|
| agy-core | core done; now the playback and preview features the tests need |
| agy-engine | the control API server, the last API difference, two-engine download |
| agy-tests | porting the 132 Go engine tests |
| agy-perf | the Direct Play save fix, the Android APK, the final speed comparison vs Go |

**Safety:** builds stay out of `/tmp`, at least 25 GB of disk is kept free, and
a governor pauses builds when free RAM drops below 10 %.
