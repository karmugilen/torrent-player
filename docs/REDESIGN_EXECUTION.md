# Authorized parallel implementation

Implementation was explicitly authorized on 2026-10-17 with the Herdr and implement
skills and the requested agent kinds agy and grok. Earlier planning-only holds
are superseded. No GitHub pushes, remote issues or publication are authorized.

The source of truth is REDESIGN_SPEC.md plus REDESIGN_TICKETS.md. The last
planning commit is `82374f4`; this execution note precedes isolated worker branches.

## Ownership

- **agy:** `android/**` only for delivered source/tests/assets. Complete UI-01–04,
  MEDIA-01, Android MEDIA-02/NET-01/CACHE-07 integration, CACHE-01/CACHE-05/CACHE-06,
  and STORAGE-01 as its own later slice. Preserve external players and world map.
- **grok:** `engine-go/**`, `engine-rust/**` only for delivered source/tests/vendor
  material. Complete REL-01, native MEDIA-02/NET-01/CACHE-07, CACHE-02/CACHE-03/CACHE-04.
  No additional Rust migration, scheduler rewrite, connection-limit or buffer tuning.
- **Lead:** shared spec/ticket state, integration, full final suites/APK, independent
  evidence verification and two-axis code review. Workers do not edit the shared
  documents or delegate further.

Workers use separate local Git worktrees/branches, stage only owned paths and
make scoped local commits. The lead integrates into `go-engine` after finished
reports. Assignment is not completion. Do not poll workers or read live reports.

## Shared interfaces

Keep existing playback JNI/control behavior intact. Additive interfaces below
allow the two platform slices to be implemented independently. Report any required
incompatible revision explicitly; do not leave a silent stub or fake active state.

### Preview reader

Android EngineHost declarations and matching native JNI exports:

```kotlin
external fun openPreview(id: String, fileIndex: Int): Long
external fun readPreview(handle: Long, offset: Long, size: Int, target: ByteArray): Int
external fun closePreview(handle: Long)
```

Open returns zero when unavailable; read returns count, zero at EOF, negative on
failure. Close is idempotent, cancels blocked reads and releases preview demand.
Preview handle ownership is separate from playback, and cannot change focus,
resume history or episode/startup ownership. Initial limits: one preview job,
8 MiB piece-aware admitted work, 10-second attempt. Whole-piece verification and
safe fallback take precedence over fabricating bytes to meet a target. Support
ready add drafts via bounded temporary storage without full destination files;
respect explicit pause/delete and configuration transitions. Network-type choice
does not introduce a Wi-Fi-only thumbnail gate.

### Experimental anti-DPI control

Use JNI's existing control transport, not a new Android control HTTP listener.

- `GET /bypass`
- `POST /bypass` with `{"enabled":true}` (or false)
- Response: `{"enabled":false,"active":false,"mode":"off","coverage":[],"error":null}`

`enabled` is requested mode; `active` represents actual working backend activation,
not an assertion of ISP effectiveness. `mode` names the technique in use; `coverage`
lists actually adapted paths (for example `outgoing-tcp-peers`, `http-trackers`,
`websocket-trackers`). Frontend persists `antiDpiEnabled`, defaults false and
reapplies after engine readiness. Failed activation is visible. Do not relabel
existing MSE configuration as a complete zapret-style feature. No custom ISP tests,
packet capture or public-swarm efficacy benchmark; user performs those trials.

### Native cache accounting and cleanup

- `GET /cache`
- `POST /cache/clear` with `{"categories":["startup","resume","peers","dht"]}`
- Response uses the GET shape:
  `{"categories":[{"id":"startup","bytes":0,"reclaimableBytes":0}],"memoryBytes":0}`

Native IDs are `startup`, `resume`, `peers`, `dht`; additive categories are allowed.
Protect active readers/verification and do not change transfer intent. Clear is
safe/idempotent; report remaining bytes rather than claiming active protected
data was erased. Android separately accounts for thumbnails, temporary imports
and media facts, and retains durable library/bookmark records.

## Authorized checks and review

Use TDD where useful at the already agreed ticket seams: library selection/order/
resume, saved-state recovery, thumbnail identity/retry, verified preview read and
cancellation, checkpoint durability, cache expiry/cleanup. These seams were
specified in the accepted tickets; no additional approval round is needed.

Workers run focused tests and regular Kotlin/native compilation for their own
changes. The lead runs full Go/Android/Rust suites once after integration, fixes
remaining failures, builds a distinct local debug APK and records its checksum.
No installing over user data or destructive device tests without a clear need.

After implementation, use the code-review skill for separate Standards and Spec
reviews in parallel against the fixed pre-implementation source point `82374f4`.
Local Markdown is the issue source; no external tracker setup is needed. Resolve
material findings before marking tickets complete. Do not claim completeness from
a worker's idle state, a successful build, or a report without verified evidence.

## Completion reports

Each worker writes its finished report to the supplied absolute path and sends
one callback to the verified parent Codex conversation. Reports contain commit
IDs, delivered tickets, exact checks, limitations and blockers; mark partial work
honestly. Do not change delivered artifacts after callback without an explicit
correction. Runtime IDs and temporary coordination material stay in ignored
`.herderHelp/`, not durable project memory.
