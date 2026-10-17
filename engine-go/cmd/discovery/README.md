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
invalid sizes are rejected. A local `torrents.csv` dump (~175MB, from the torrents-csv project) is the main
fast name path: hash→name lookup with no peer upload. It refreshes about once a
day. A paced remote catalog API still adds browseable names. Already-resolved
hashes are never re-checked. Name-resolve scheduling scores unresolved hashes by
seeds, peers, repeated DHT samples, and real retry overdue time so quiet noise
does not starve hot swarms. The live names/min metric counts only catalog, cache,
and peer wins (dump fills of already-sampled hashes are shown separately). DHT
sampling stays quiet for peer geography only. Movie payload download and upload
stay disabled. The JSON store has no count cap. The detail drawer can copy a
magnet link for download in another client.

The resolver takes inspiration from the sibling `seen-in-the-swarm` project:
parallel sources, bounded requests, source cooldowns and scheduled retries.
Twenty-four cache workers and six peer workers stay small on purpose so the
uplink is not clogged. Cache workers only query HTTPS `.torrent` mirrors and
release immediately on a miss. Peer workers handle BEP 9 for cache misses only.
Four sampling slots refill independently of slow nodes. Random routing lookups
refresh discovery every three seconds. Advertised sampling intervals remain
respected. A DHT limit of 24 sends/second and peer dial limit of 12/second keep
traffic light. HTTP connections are reused, with up to 12 requests per cache
source. Challenge HTML and ordinary misses leave mirrors healthy; only transport
failures and rate limits back them off. Names appear immediately from catalog or
cache hits. Cache deadlines are 2.5 seconds. Losing HTTP requests are canceled
and joined before a peer job finishes. Completed empty lookups release their
slot as soon as cache and source checks finish. Lookups with no peer sightings
or active connections stop after eight seconds and remain eligible for scheduled
retries; live peer lookups retain the twelve-second deadline.

A bounded RAM cache retains up to 24 peer addresses for each of 2,048 swarms for
five minutes. Retries can dial these hints immediately while DHT checks run.
Hints are not saved, exposed through the API, or counted as new map sightings.
DHT bootstrap runs before resolver workers and retries automatically after a
cold-start failure, so a full metadata queue cannot prevent discovery starting.

Unresolved hashes retry automatically after 30 seconds, doubling to at most 15
minutes; retries do not require another sample. One in four dispatches is reserved
for due retries. Reachable unresolved swarms survive pruning ahead of untested
hashes. Cached names are reused and their availability is checked every 30 minutes.
The page shows active/due jobs, new names per minute, catalog/cache/peer wins,
successful names versus completed jobs, and mean lookup time. These counters
describe the current process; retained names can include earlier sessions.

This is primarily network-bound work. Go goroutines already run concurrently and
use multiple CPU cores. A Rust rewrite, GPU compute or deep packet inspection is
not required for this resolver. Payload download/upload is disabled and the
storage implementation independently rejects movie-data reads and writes. The
page reports the actual torrent payload counter.

The JSON store has no count cap: every named and pending observation is retained
until age expiry. Named rows reload for 30 days; unresolved DHT noise expires
after 24 hours. Storage is a plain JSON file only (no SQL) under
`~/.cache/torrent-player/discovery.json` (or `$XDG_CACHE_HOME`). A local copy of
the collaborative `torrents.csv` dump (~175MB, refreshed about monthly) lives
beside it as `torrents.csv` and resolves DHT hashes to names with no peer upload.
The remote torrents-csv search API still seeds browseable names. Raw peer
addresses are transient and are not exposed or saved. The page can search stored
titles, show today/week trending, rank by seeding, and hide unlocated countries.
An observation is not proof that someone downloaded or watched a file.

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

The sampling pipeline update was checked separately: sampling replies rose from
about 39/minute over a 105-second window to 121/minute over a 36-second window.
The live retained count grew past 3,000. These short sequential observations also
depend on the responding DHT nodes. Regression checks cover growth beyond 600,
independent sampling-slot refill, interval handling and candidate priority.

A subsequent name-focused check observed 12.6 newly resolved names/minute and
64 completed jobs/minute over 182 seconds before the resolver tuning, versus
18.1 names/minute and 130 completed jobs/minute over 66 seconds afterward. The
latter window completed 20 new names, reused peer hints five times, and recorded
zero payload bytes. Public-network availability makes these variable; job
throughput and actual named results are measured separately. Tests additionally
cover prompt completion of an empty real local DHT lookup and hint-cache bounds,
expiry, deduplication and address isolation. Browser checks include unknown
country labels so an unlocated peer cannot blank the result table.

After adding the paced torrents-csv catalog seeder, a 60-second live window
recorded about 14,000 new catalog names/minute before pacing was corrected to
target 1,000 names/minute and to stop filling beyond 70% of the 10,000-slot
retention so DHT samples still fit. Cache and peer resolvers remain active for
hashes the catalog does not name. Catalog ingest unit tests cover naming,
seed/peer counts, video kind detection and duplicate suppression.


The current webpage uses fixed-size Next/Previous pages, source filters, shared
URL state, and cancellation of stale search requests. Searches accept words in
any order, accents, punctuation, hashes and magnets. Live names, index additions,
verified metadata and completed checks are measured over the last 60 seconds.
The 1,000/minute goal counts fresh names, never a replay of the stored catalog.

Rust now provides compact token/prefix search postings and parallel pending-hash
lookup in batches. The Go fallback remains supported. A 100,000-row browse test
reduced request allocations from 27.3MB/300,157 allocations to 0.25MB/616 and
elapsed time from 100ms to 54ms. A live unknown-title search on the million-row
store fell from about 1,014ms to 21ms; loaded/active-state timings vary. Only
selected result rows are deep-copied. Cache misses hand directly to peer workers
instead of being reclaimed by the cache pool. Full-store persistence runs every
two minutes plus explicit saves, with one writer at a time.
