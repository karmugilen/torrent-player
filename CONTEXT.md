# Domain glossary

Short terms for this repo. **Discovery (Torrent Atlas) detail:** point agents at [`engine-go/cmd/discovery/CONTEXT.md`](engine-go/cmd/discovery/CONTEXT.md) for run/build, dump/Rust resolve, API windowing, search, and file map.

## Discovery (local metadata atlas)

- **Observation**: one infohash row in the local JSON store (name, peers, countries, source).
- **Name apply**: single merge path that turns a dump/catalog/cache/peer name into an Observation.
- **Pending set**: index of unresolved Observations used for resolve scheduling (not the full store).
- **Dump index**: local torrents.csv hash→name lookup; does not own store mutation.
- **Windowed view**: ranked subset of Observations sent to the browser; full store stays on disk.
- **rustdump**: build tag linking the Rust mmap dump/catalog crate into the discovery binary.
