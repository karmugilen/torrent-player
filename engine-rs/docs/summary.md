## In plain words (updated 16:20)

**Where we are:** all the Rust code is written and it all compiles. Six of the
8 parts are finished and tested. The last two, the torrent core (`wt-core`) and
the engine/control API (`wt-engine`), work for basic downloads but still have
bugs in some cases.

**What works today:** two Rust clients can download a file from each other
over TCP and verify every byte. The Android `libengine.so` builds. 29 of the
31 control-API answers match the Go engine.

**What is left (7 of 13 milestones):**

1. Magnet links and seeking on the new connection code (a stall bug is being fixed)
2. Two full engines downloading from each other through the control API
3. The last API answer that differs from Go (`/metadata`)
4. Port the 132 remaining Go engine tests for real (11 done)
5. Move a slow Direct Play save off the storage lock (Go does this already)
6. Speed/RAM/CPU comparison against Go: Rust must match or beat it
7. Build the Rust APK and test it on a phone (needs a phone attached by USB)

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
