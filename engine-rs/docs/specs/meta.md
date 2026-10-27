# Task: wt-meta (bencode + metainfo + magnets + merkle) — pure Rust port
Target: engine-rs/crates/wt-meta/ only.
Change: Port engine-go/patches/torrent/{bencode,metainfo,merkle} and infohash v1/v2 types to Rust, producing the wt-types data model (Info, FileInfo, MetaInfo, Magnet, InfoHash, InfoHashV2).
Required public API (publish this skeleton FIRST, compiling, then implement):
- `pub mod bencode` with `Value` (Int, Bytes, List, Dict as ordered BTreeMap<Vec<u8>,Value>), `decode(&[u8]) -> Result<Value>`, `decode_prefix(&[u8]) -> Result<(Value, usize)>` (for ut_metadata trailing data), `encode(&Value) -> Vec<u8>`, plus raw span lookup so the exact `info` bytes can be extracted from a .torrent.
- `parse_metainfo(&[u8]) -> Result<MetaInfo>`, `encode_metainfo(&MetaInfo) -> Vec<u8>`, `parse_info(&[u8]) -> Result<Info>`, `encode_info(&Info) -> Vec<u8>`.
- `info_hash_v1(info_bytes) -> InfoHash`, `info_hash_v2(info_bytes) -> InfoHashV2`.
- `parse_magnet(&str) -> Result<Magnet>` (btih hex/base32, btmh multihash, tr, dn, ws/xs/as, x.pe), `magnet_uri(&Magnet) -> String` matching Go's output.
- `pub mod merkle`: v2 piece hashing, root from layer, verify a piece against piece layers, BEP 52 file tree parsing into FileInfo with pieces_root and torrent_offset (with padding semantics like Go), piece layers validation.
- Padding files (BEP 47), path.utf-8 preference, name.utf-8, single-file form, private flag, url-list string-or-list, announce-list, nodes, piece length validation exactly as Go.
Constraints: no unsafe; zero-copy where practical; fuzz-robust (never panic on malformed input — return Err). Lenient where Go is lenient (ignore_unmarshal_type_error fields).
Observable acceptance: port the Go tests in bencode/*_test.go, metainfo/*_test.go and merkle tests using the testdata/*.torrent files (copy needed testdata into your crate); infohashes of the bundled testdata torrents equal Go's; magnet round-trips match Go. Add a cargo-fuzz-free proptest/quickcheck-style round-trip test for bencode.
