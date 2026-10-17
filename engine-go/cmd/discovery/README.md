# Torrent Atlas — local webpage

A small desktop page for public DHT discovery. It shows a world map, searchable
names, file metadata, observed peer counts, country filters and a top-ten view of
the local sample. It runs on `http://127.0.0.1:18181`; nothing is published.

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
hashes. It first contacts the node that supplied a hash, so metadata connections
can begin while the broader lookup continues. BEP 9 metadata supplies verified
torrent names and file lists. Names are displayed as text, not executable HTML.

Four metadata lookups can run at once, with a fourteen-second deadline and a
shared DHT limit of twelve sends/second. Payload download/upload is disabled;
the storage implementation independently rejects all movie-data reads and
writes. The page reports the actual torrent payload counter.

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
bounded retention, metadata escaping and control request validation. Desktop and
mobile browser checks cover the real map, search, details, ranking and filters.
