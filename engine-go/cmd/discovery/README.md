# Torrent Atlas — local webpage

A small desktop page for public DHT discovery. It shows a world map, searchable
names, file metadata, observed peer counts, country filters and a top-ten view of
the local sample. Search handles different word order and separators, file names,
accent-insensitive text and v1 magnet links. It runs on `http://127.0.0.1:18181`; nothing is published.

From the repository root:

```sh
./scripts/discovery.sh start
xdg-open http://127.0.0.1:18181
```

Use `./scripts/discovery.sh stop` to stop the local process. The page also has a
pause/resume button for network collection. On Linux the launcher prefers a
transient user systemd process, which does not enable startup at login. Go 1.24+
is required to build the collector.

The page and map use no external JavaScript, fonts or map tiles. Country lookup
uses the existing offline country database under `android/app/src/main/assets`;
its attribution is in `android/app/src/main/assets/licenses/peer-map-data.txt`.
Unknown locations remain unlocated. No Android changes or phone installation are
needed.

## How observations work

The collector asks public DHT nodes for BEP 51 samples, honors the returned
sampling interval, and looks up peers and BEP 33 seed estimates for sampled
hashes. It races up to three recently observed source nodes with the broader
DHT traversal, so a slow source cannot block peer discovery. BEP 9 metadata and
public HTTPS `.torrent` caches race to supply names. Cache replies must match the
exact SHA-1 infohash; private torrents, HTML responses, oversized metadata and
invalid sizes are rejected. Movie payload requests remain disabled.

The resolver takes inspiration from the sibling `seen-in-the-swarm` project:
parallel sources, bounded requests, source cooldowns and scheduled retries.
Twelve asynchronous jobs run at once across the Go runtime's available CPU cores.
A shared DHT limit of 32 sends/second and peer dial limit of 32/second bound network
work. HTTP connections are reused, with at most three requests per cache source.
Names appear immediately; availability counts finish settling afterward. Deadlines
are twelve seconds for a job and four seconds for HTTP cache races. Losing HTTP
requests are canceled and joined before a job finishes.

Unresolved hashes retry automatically after 30 seconds, doubling to at most 15
minutes; retries do not require another sample. One in four dispatches is reserved
for due retries. Reachable unresolved swarms survive pruning ahead of untested
hashes. Cached names are reused and their availability is checked every 30 minutes.
The page shows active/due jobs, new names per minute, cache/peer wins, successful
names versus completed jobs, and mean lookup time. These counters describe the
current process; retained names can include earlier sessions.

This is primarily network-bound work. Go goroutines already run concurrently and
use multiple CPU cores. A Rust rewrite, GPU compute or deep packet inspection is
not required for this resolver. Payload download/upload is disabled and the
storage implementation independently rejects movie-data reads and writes. The
page reports the actual torrent payload counter.

Observations are bounded to 600 retained hashes, favoring resolved names. Only
torrent metadata and aggregate country counts are cached, under
`~/.cache/torrent-player/discovery.json` (or `$XDG_CACHE_HOME`). Raw peer addresses
are transient and are not exposed by the page or saved in that cache. Cache
loading discards observations older than 24 hours. Restarting the collector
retains names; fresh network checks update availability. An observation is not
proof that someone downloaded or watched a file.

“Top 10 observed” ranks the matching local observations by distinct peer IPs
found per torrent. A peer can occur in multiple torrents. DHT samples do not
represent worldwide popularity; unresolved names and unavailable seed estimates
are explicitly shown. Adult titles can be concealed with the page's name filter.

## Verification

```sh
cd engine-go
go test -race -tags=nosqlite,noboltdb ./cmd/discovery
go vet -tags=nosqlite,noboltdb ./cmd/discovery
```

Tests cover payload rejection, cached observation recovery, snapshot isolation,
bounded retention, metadata escaping, control request validation, exact cache hash
verification, private torrent rejection, concurrent mirror cancellation, healthy
cache misses, outage backoff and retries without new DHT announcements. Desktop and
mobile browser checks cover the real map, search, details, ranking and filters.

A local live check on 2026-10-17 observed about 18 lookup starts/minute over
144 seconds before this change, and 62 completed lookups/minute over 83 seconds
after it. The latter window resolved eight additional names and recorded zero
payload bytes. These are short, sequential public-network observations, not a
controlled benchmark or a guaranteed resolution rate.
