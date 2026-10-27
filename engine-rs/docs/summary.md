## In plain words (updated 16:35)

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

1. Two full engines downloading from each other through the control API. Each
   side connects, but the download stays at 0 %. This is the main bug now.
2. Port the 128 remaining Go engine tests for real (14 pass)
3. Speed/RAM/CPU comparison against Go: Rust must match or beat it. The one
   speed number so far (2.4 MiB/s) came from an unoptimized debug build, so it
   does not count; it is being re-measured in release.
4. Build the Rust APK and test it on a phone (needs a phone attached by USB)

**Who is doing it:** 4 Antigravity agents, each on its own part, working on
their own. The machine rebooted at 16:05, which closed the earlier Codex
agents. Their work was saved and committed before the new agents started.

| Agent | Doing |
|---|---|
| agy-core | fixing the download stalls, uTP, encryption, magnet, seek, speed vs Go |
| agy-engine | the control API server, the last API difference, two-engine download |
| agy-tests | porting the 132 Go engine tests |
| agy-perf | the Direct Play save fix, the Android APK, the final speed comparison vs Go |

**Safety:** builds stay out of `/tmp`, at least 25 GB of disk is kept free, and
a governor pauses builds when free RAM drops below 10 %.
