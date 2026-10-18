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
does not starve hot swarms. The live names/min metric counts cache/peer wins and
new names for DHT-observed hashes; bulk catalog additions are shown separately. DHT
sampling remains bounded alongside incoming peer announcements. Movie payload download and upload
stay disabled. The JSON store has no count cap. The detail drawer can copy a
magnet link for download in another client.

The resolver takes inspiration from the sibling `seen-in-the-swarm` project:
parallel sources, bounded requests, source cooldowns and scheduled retries.
Ninety-six cache workers and twenty-four peer workers run with bounded network
admission. Cache workers only query HTTPS `.torrent` mirrors and
release immediately on a miss. Peer workers handle cache misses and fast-track
fresh endpoints or feed-associated tracker/publisher metadata hints.
Four sampling slots refill independently of slow nodes. Random routing lookups
refresh discovery every three seconds. Advertised sampling intervals remain
respected. A DHT limit of 64 sends/second and peer dial limit of 24/second bound
network traffic. HTTP connections are reused, with up to 48 requests per cache
source. Confirmed 404/410 responses leave mirrors healthy; challenge HTML, HTTP
403, transport failures, rate limits and server errors back them off. Names appear immediately from catalog or
cache hits. Cache deadlines are 2.5 seconds. Losing HTTP requests are canceled
and joined before a peer job finishes. Completed empty lookups release their
slot as soon as cache and source checks finish. Lookups with no peer sightings
or active connections stop after three seconds and remain eligible for scheduled
retries; live peer lookups retain the twelve-second deadline.

A bounded RAM cache retains up to 24 peer addresses for each of 2,048 swarms for
five minutes. Retries can dial these hints immediately while DHT checks run.
Hints are not saved, exposed through the API, or counted as new map sightings.
DHT bootstrap runs before resolver workers and retries automatically after a
cold-start failure, so a full metadata queue cannot prevent discovery starting.

Unresolved hashes retry automatically after 30 seconds, doubling to at most 15
minutes; retries do not require another sample. One in four dispatches is reserved
for due retries. Reachable unresolved swarms survive pruning ahead of untested
hashes. Named hashes leave the resolver queue and are reused without network rechecks.
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
the collaborative `torrents.csv` dump (~175MB, checked hourly and fetched when the local copy is over 24 hours old) lives
beside it as `torrents.csv` and resolves DHT hashes to names with no peer upload.
The remote torrents-csv search API still seeds browseable names. Raw peer
addresses are transient and are not exposed or saved. The page can search stored
titles, show today/week trending, rank by seeding, and hide unlocated countries.
An observation is not proof that someone downloaded or watched a file.

“Top 10 observed” ranks the matching local observations by distinct peer IPs
found per torrent. A peer can occur in multiple torrents. DHT samples do not
represent worldwide popularity; unresolved names and unavailable seed estimates
are explicitly shown. Adult titles can be concealed with the page's name filter.

## Bulk lookup and fresh inputs

Open **Bulk hash → name lookup** to paste hashes/magnets or select a text/CSV
file with up to 100,000 unique hashes. Results show verification provenance, a
bounded preview and CSV export. Hex and base32 aliases are deduplicated. Work
continues in background tabs and can be cancelled. Existing names are looked up
locally; this does not inflate new-name or network-check counters.

`POST /api/resolve` accepts `{"hashes":["…"]}` (at most 50,000 inputs / 4 MiB,
two concurrent requests). The page sends 10,000-hash batches. Missing names are
returned explicitly rather than replaced by provisional feed titles.

SubsPlease, Nyaa and EZTV feeds refresh every five minutes using conditional
requests. Feed labels stay provisional until catalog matching or exact metadata
verification. Associated tracker URLs and publisher `.torrent` links help find
metadata. Tracker requests respect per-hash intervals, reuse UDP clients and
resolve only public IPv4 endpoints. PEX is enabled. A normal DHT node listens on
UDP 18182 (or an ephemeral port if busy); its announcement counter reports actual
inbound traffic. Inbound traffic still depends on network reachability.

To override the feed list, set `TORRENT_ATLAS_FEEDS` before starting the process;
an empty value disables feeds. Additional CSV or CSV.GZ datasets placed in
`~/.cache/torrent-player/catalogs/` are streamed into the shared name/search store
within five minutes. Supported headers are `infohash`/`hash`/`info_hash`,
`name`/`title`, and optional `size_bytes`/`size`. Catalog labels do not replace
verified names. `--catalog-dir` selects another folder when invoking the binary.

Per-mirror requests, hits, misses, failures, cancellations and approximate p95
latency are exposed in `metadataSources`. Tracker and feed counters are separate.
Snapshot saves and startup restoration stream records instead of constructing
multiple full copies of the JSON database in memory.

### Measured indexed capacity (2026-10-18)

On this Ryzen 5 5600H PC, the running HTTP endpoint returned names for 100,000
distinct hashes sampled across the actual CSV in **0.3582 seconds**, including
response transfer and client JSON decoding. The real browser file-upload flow
matched 100,000 distinct hashes in **0.55 seconds**, including the UI's sequential
batch requests. These demonstrate the 100,000/minute indexed-capacity target;
they reuse existing labels and do not demonstrate that many fresh network names.

The initial live feed backlog reached 106 fresh names/minute in one short rolling
window. A later window fell to 8/minute as the backlog drained. Publisher
metadata returned 73 verified results during that session; payload bytes stayed
zero. These are variable public-network observations, not sustained guarantees.

A controlled 100,000-record save benchmark reduced allocation from about
360 MB to 64 MB per save and elapsed time from 370 ms to 293 ms. The streamed
CSV-import fixture processed 100,000 distinct new catalog labels in 274 ms.
These fixture timings exclude remote source delivery.

Reproduce indexed capacity through the actual HTTP interface:

```sh
python scripts/benchmark-discovery.py
```

## Verification

```sh
cd engine-go
go test -race -tags=nosqlite,noboltdb ./cmd/discovery
go test -race -tags=nosqlite,noboltdb,rustdump ./cmd/discovery
go vet -tags=nosqlite,noboltdb,rustdump ./cmd/discovery
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

### Naming reliability and queue efficiency

The upstream [torrents-csv contribution process](https://codeberg.org/heretic/torrents-csv-data/src/branch/main/README.md)
scans contributors' existing `.torrent` files, checks seeder availability, merges
their metadata into a database, and exports the CSV. Names come from collected
metadata; a tracker scrape supplies availability counts, not a name from a hash.
Our collector also obtains metadata through public caches and BEP 9 peers.

Workers now choose from bounded batches rather than walking the full unresolved
backlog for every job or idle poll. Batches refresh every 250ms, and each
dispatch revalidates resolution, backoff, and in-flight ownership. Cache misses
immediately wake the peer batch. One cache/peer cycle counts as one attempt, so
the first failed cycle retries after 30 seconds instead of skipping to 60.
Recently reachable swarms take priority over old empty retries; each pool still
reserves one dispatch in four for due retries.

When all mirrors are cooling down, cache workers wait rather than marking hashes
as misses. If sources become unavailable after dispatch, that skipped work does
not inflate completed lookups or consume a retry attempt. Healthy cache misses
still hand off immediately to peers.

An unchanged CSV is checked at most once per pending hash. A refreshed index
invalidates those misses automatically. Rust uses a complete JSON parser for
catalog names, including Unicode escapes, quotes and braces; malformed JSON and
HTML trigger fallback instead of being treated as successful empty pages. An
in-flight exact metadata result can correct a catalog title without increasing
the new-name count twice. Existing named rows do not start new verification jobs.

The synthetic 100,000-hash waiting-backlog benchmark measured roughly 42ms per
dispatch before batching versus 0.15ms afterward, including periodic refreshes
under a simulated worker polling clock. This measures scheduling overhead, not
public-network metadata availability or a guaranteed new-name rate.

Cache admission now reserves an entire hash before starting its 2.5-second
network deadline. Up to 48 hashes can race the three mirrors concurrently;
waiting hashes retain their request budget, and cancellation interrupts admission.
Each mirror remembers confirmed HTTP 404/410 misses for two minutes in a bounded
8,192-entry RAM cache. Cached misses can hand off to peers, but do not count as
new network lookups. HTML challenges, errors and timeouts are never negative
cached. The API exposes `cacheRequests`, `cacheMissReuses`, `cacheActive` and
`cacheCapacity` for checking actual HTTP activity and capacity use.

A controlled batch of 96 hashes against three 40ms local mirrors took roughly
330ms before these changes and 85ms afterward (about 3.9x throughput). This is a
transport benchmark; live results depend on source availability and throttling.

On 2026-10-18, sequential live windows showed 1,239–1,310 completed lookups/min
before this tuning and 4,230–5,450 afterward. The final window resolved four new
verified names/min and transferred zero torrent payload bytes. Cached misses
were not counted as network lookups. These public-network samples are variable;
the controlled local benchmark above isolates the transport improvement.

Rust now provides compact token/prefix search postings and parallel pending-hash
lookup in batches. The Go fallback remains supported. A 100,000-row browse test
reduced request allocations from 27.3MB/300,157 allocations to 0.25MB/616 and
elapsed time from 100ms to 54ms. A live unknown-title search on the million-row
store fell from about 1,014ms to 21ms; loaded/active-state timings vary. Only
selected result rows are deep-copied. Cache misses hand directly to peer workers
instead of being reclaimed by the cache pool. Full-store persistence runs every
two minutes plus explicit saves, with one writer at a time.
