# api-diff CRITICAL diffs (for agy-4)

From final session run: **11 MATCH · 19 DIFF · 14 CRITICAL**

Report: `crates/wt-parity/API_DIFF.md`

## Critical list

- GET /stats: CRITICAL progress: type go=number/int rust=number/float; CRITICAL ratio: type go=number/int rust=number/float
- GET /torrent/<id>: CRITICAL files[0].name: value go="parity-video.bin" rust="file.mp4"; CRITICAL files[0].path: value go="parity-video.bin" rust="file.mp4"; CRITICAL files[0].progress: type go=number/int rust=number/float; CRITICAL files[0].type: value go="application/octet-stream" rust="video/mp4"; CRITICAL length: value go=0 rust=1048576; CRITICAL name: value go="parity-video.bin" rust="test_torrent"; CRITICAL progress: type go=number/int rust=number/float; CRITICAL seedRatio: type go=number/int rust=number/float; CRITICAL trackers: array length go=0 rust=1
- POST /play: CRITICAL name: value go="parity-video.bin" rust="file.mp4"
- GET /pieces/<id>: CRITICAL buckets: array length go=4 rust=10; CRITICAL ghostProMode: value go=false rust=true; CRITICAL lastPieceLength: value go=262144 rust=16384; CRITICAL pieceLength: value go=262144 rust=16384; CRITICAL totalPieces: value go=4 rust=10
- GET /metadata/<id>: CRITICAL torrentData: value go="ZDc6Y29tbWVudDI4OmR5bmFtaWMgbWV0YWluZm8gZnJvbSBjbGllbnQxMDpjcmVhdGVkIGJ5MzY6aHR… rust="ZDc6Y29tbWVudDE3Ond0LXBhcml0eSBmaXh0dXJlMTA6Y3JlYXRlZCBieTk6d3QtcGFyaXR5NDppbmZ…
- GET /settings: CRITICAL arrival.networkErrMs: type go=number/int rust=number/float; CRITICAL dialRatePerSec: value go=30 rust=50; CRITICAL ghostProMode: value go=false rust=true; CRITICAL idleDropMinutes: value go=0 rust=15; CRITICAL seedingEnabled: value go=false rust=true
- POST /settings: CRITICAL arrival.networkErrMs: type go=number/int rust=number/float; CRITICAL dialRatePerSec: value go=30 rust=50; CRITICAL ghostProMode: value go=false rust=true; CRITICAL idleDropMinutes: value go=0 rust=15
- POST /add (magnet): CRITICAL infoHash: value go="34604a6ebe97b36d1b610eabd84259f048adf961" rust="magnet:?xt=urn:btih:34604a6ebe97b36d1b610eabd84259f048adf961&dn=parity-video.bi…
- GET /preview-state/<id>: status go=Some(400) rust=Some(200); error: present in go, missing in rust; CRITICAL active: present in rust, missing in go; CRITICAL byteOffset: present in rust, missing in go; CRITICAL signature: present in rust, missing in go
- GET /bypass: CRITICAL error: go null vs rust missing; CRITICAL coverage: go null vs rust array
- POST /bypass: CRITICAL error: go null vs rust missing; CRITICAL coverage: go null vs rust array
- GET /cache: CRITICAL categories: array length go=4 rust=2
- POST /cache/clear: status go=Some(400) rust=Some(200); error: present in go, missing in rust; CRITICAL categories: present in rust, missing in go; CRITICAL memoryBytes: present in rust, missing in go
- POST /play (bad: missing id): status go=Some(404) rust=Some(400); both 4xx but codes differ
