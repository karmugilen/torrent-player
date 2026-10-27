# wt-meta port notes

Pure Rust port of `engine-go/patches/torrent/{bencode,metainfo,merkle}` plus
infohash v1/v2 hashing. Consumes `wt-types` for `Info`, `FileInfo`, `MetaInfo`,
`Magnet`, `InfoHash`, `InfoHashV2`.

## Ported

### `bencode`
- `Value::{Int, Bytes, List, Dict(BTreeMap<Vec<u8>, Value>)}`
- `decode`, `decode_prefix` (ut_metadata trailing payload), `encode`
- `extract_dict_key_raw` for exact `info` byte spans (infohash input)
- Fuzz-robust: malformed input returns `Err`, never panics
- Lenient dict key order on decode (stored sorted in `BTreeMap`)
- Quickcheck round-trip + random-bytes non-panic tests

### `metainfo`
- `parse_metainfo` / `encode_metainfo` (exact `info_bytes` splice)
- `parse_info` / `encode_info`
- `info_hash_v1` (SHA-1) / `info_hash_v2` (SHA-256)
- BEP 47 padding via `attr` containing `p`
- `path.utf-8` / `name.utf-8` preference (folded into `path` / `name`)
- Single-file `length` form, `private`, `url-list` string-or-list
- `announce-list`, `nodes` (string or `[host, port]`, empty string ignored)
- `creation date` with ignore-type-error semantics
- BEP 52 `file tree` → `FileInfo` with `pieces_root` and piece-aligned
  `torrent_offset` (Go padding semantics)
- `piece layers` parse + `validate_piece_layers`
- `choose_piece_length` (Go `ChoosePieceLength`)

### `magnet`
- `parse_magnet`: `btih` hex/base32, `btmh` multihash, `tr`, `dn`,
  `ws`/`xs`/`as` → `web_seeds`, `x.pe` and other keys → `params`
- `magnet_uri`: Go `MagnetV2.String()` layout (unescaped `xt=` first, then
  sorted `url.Values`-style query)

### `merkle`
- Block size 16 KiB, `root`, `root_with_pad_hash`, `compact_layer_to_slice_hashes`
- Incremental `Hash` with `sum` / `sum_min_length`
- `hash_for_piece_pad`, `validate_piece_layers`

### Tests / testdata
- Copied Go `.torrent` fixtures under `testdata/`
- Infohashes of all bundled torrents match Go
- Magnet URI outputs match Go for archive.org url-list cases
- v2 / hybrid piece-layer validation passes
- v1 `encode_info` exact byte match for Go `TestFile` torrents

## Differs from Go

- No reflection-based generic marshaler; API is `Value`-centric plus typed
  metainfo helpers (as required by the crate contract).
- Integers that do not fit in `i64` return `Error::IntegerOverflow` (Go uses
  `math/big`). Torrent fields never need bigger ints.
- `wt-types::Info` has a single `name` / `FileInfo.path`; UTF-8 variants are
  preferred at parse time and not stored separately, so lossless re-encode of
  torrents that carry both `name` and `name.utf-8` is not guaranteed.
- Dict key sort order violations are accepted on decode (Go is also lenient).
- Creating torrents from a filesystem (`BuildFromFilePath`, `GeneratePieces`)
  is not ported; piece-length choice is available.

## Remains / out of scope here

- Wire-level ut_metadata message framing (`wt-wire`)
- Storage-backed piece hashing beyond the merkle helpers
- Optional: preserve `name.utf-8` / `path.utf-8` as distinct fields in
  `wt-types` if lossless hybrid re-encode of those keys becomes required
