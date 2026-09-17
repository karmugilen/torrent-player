# Faster preparation and user experience

Date: 2026-09-16

## Findings

- New magnets need reachable peers to supply metadata; additional UI work cannot
  guarantee a shorter network wait. Saved metadata is already reused on restore.
- Restoring selected files currently queries/opens them for accessibility, then
  opens them again for configuration. Unselected saved files are also opened.
- Resume verification reads/hash-checks the full selected size, including sparse
  holes created when download files are sized before bytes arrive.
- The metadata waiter waits for an event before checking already available data.

## Implement now

1. [x] Open selected files once and use those handles for access validation and
   configuration. Keep cleanup and missing-file errors, and leave saved files intact.
2. [x] Check metadata immediately before waiting for a change notification. Keep the
   existing deadline, cancellation and peer discovery behavior.
3. [x] During saved-data verification only, use filesystem-confirmed empty regions
   to compute the exact zero-data hash once per piece size. Continue normal
   hashing for actual data, unsupported filesystems, mixed pieces, v2/hybrid
   torrents and any uncertain result. Preserve normal peer-data hash checking
   and smart banning outside restore.
4. [x] Test partial, complete, corrupt, zero-filled, truncated and shared-boundary
   files. Benchmark sparse and fully populated files; report local measurements,
   not an assumed phone speedup. Run existing regression/build checks.

### Local sparse benchmark (1 MiB piece, Linux tmpfs/ext4-style holes)

- Sparse all-hole `SelfHash`: ~336 ns/op (~3.1 TB/s effective)
- Dense full-hash `SelfHash`: ~1.06 ms/op (~0.99 GB/s)
- Existing Go `./...` tests and Android unit tests pass after the change.

Filesystem reference: https://man7.org/linux/man-pages/man2/lseek.2.html.
Providers may not expose holes; full verification remains the fallback.

## Product priorities (proposals, not implemented in this pass)

1. **Clear progress stages:** Finding peers, Fetching file list, Opening files,
   Checking saved data and Downloading, with an actionable reason when stalled.
2. **Download queue:** User-defined order and a small active-download limit so
   many torrents do not compete for storage and bandwidth.
3. **Network controls:** Wi-Fi-only downloads, upload/download limits and optional
   schedules, with visible reasons for automatic pauses.
4. **Playback readiness:** Show buffer readiness for the selected file; allow
   prioritizing that file without changing other downloads' selections.
5. **Storage recovery:** Locate moved files, renew folder access and report low
   space before a write fails. Keep remove-from-library separate from delete-files.
6. **Optional connection diagnostics:** Tracker errors, metadata state and useful
   peer counts, plus a redacted diagnostic export for bug reports.

Persistent completion checkpoints may further speed resume, but are deferred
until file changes, interrupted writes and power loss can be handled correctly.
