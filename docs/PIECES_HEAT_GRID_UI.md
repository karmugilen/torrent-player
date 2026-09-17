# Pieces heat grid UI

Design note for replacing the current single-row Pieces strip on torrent Details with a **soft-tile heat grid + mixed-fill**. Use this when designing or implementing the Pieces map UI.

**Status:** Implemented (2026-09-16). Execution contract below takes precedence over earlier design alternatives.  
**Screen:** Torrent Details (`TorrentFilesScreen`) — section under Transfer / Connection.  
**Related:** `docs/DOWNLOAD_UX_SPEC.md` §5 (Download details and live piece map).  
**Current code:** `PieceHeatGridSection` (`PieceHeatGrid.kt`) + pure helpers (`PieceHeatGridModel.kt`); wired from `TorrentFilesScreen.kt`. Old `PiecesSection` / `PieceCanvas` removed.  
**Data:** `EngineClient.pieces(id, maxBuckets)` → `PieceTelemetry` / `PieceBucket` with additive `selectedVerified` / `selectedReceiving`.

---

## Execution plan — 2026-09-16

Requested scope: remove the concurrent-download setting, Off/1/2/3 slot queue,
ordering controls and automatic slot holds. Eligible downloads may run together;
manual pause, file selection, network policy, speed limits and retry behavior remain.
Ignore/remove old queue preferences on upgrade so a previous slot limit cannot
silently hold downloads. Do not change swarm connections or tracker discovery.

Implement the Pieces visualization described here (this is download piece progress,
not a map of remote peers): bounded soft-tile Canvas, proportional mixed fills,
counted color legend, percentage, piece size, empty state, tap details and file
boundary markers where file ranges are available. Keep motion static for this
version. Retain foreground-only updates and cap telemetry reads at once per second.

Contract correction from source inspection: bucket `start` and `end` are inclusive
zero-based piece indices. Existing `verified` and `receiving` count **all** pieces,
including excluded files; dividing these fields by `selected` can overstate progress.
Add `selectedVerified` and `selectedReceiving` counts without changing existing
fields. Render selected verified / selected receiving / selected missing / excluded
portions against `total`. Use selected verified divided by selected for the title
(show no selected pieces when zero); legend counts use these disjoint categories.
Excluded-only cells remain hollow even if saved bytes are verified. Receiving means
partial/checking/hashing pieces, not a guaranteed live network transfer. A boundary
marker indicates the piece containing a file start; files can share a piece.

Delegation and acceptance:

1. Grok `queue_remove`: remove queue UI, state, scheduler and dead tests; preserve
   network restrictions and user pause intent. Own existing app files except the
   old Pieces drawing section; preserve unrelated work in this dirty checkout.
2. Grok `pieces_grid`: create a separate heat-grid component and pure geometry/count
   helpers with focused tests. Own new grid files only. Lead wires it into Details.
3. Grok `piece_counts`: additive selected counts in Go and Kotlin API plus regression
   tests for mixed selection and inclusive ranges. Own engine and core API files.
4. Lead: integrate Details, review worker reports, run Go and Android tests/lint,
   build a signed test APK, and install if the phone is available. Record actual
   validation and any remaining device-only checks. No commit/release in this task.

Optional selected-only filtering and animated pulses remain future polish.

---

## 1. Goal

Make the Pieces map feel like classic torrent “blocks,” stay honest for mixed buckets, and stay readable on Material 3 mobile — without engine API changes.

**Locked direction**

| Decision | Choice |
|----------|--------|
| Layout | Wrapping **heat grid** (not a single horizontal strip) |
| Tile style | **Soft rounded tiles** + small gaps |
| Mixed buckets | **Mixed-fill / split** inside one cell (never paint fully verified) |
| Legend | **Color chips + counts** (not text-only) |
| Receiving motion | Optional subtle pulse; **frozen** when paused or complete |

---

## 2. Current UI (baseline)

Today Details shows:

```
Pieces
┌────────────────────────────────────────────┐
│ ▓▓▓▓▓▓▒▒░░░░░░░░▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ │  ← one 24dp strip
│ Missing · Receiving · Verified · Excluded  │  ← text legend, no counts
└────────────────────────────────────────────┘
```

**Problems**

- One solid color per bucket → mixed ranges look fully verified or fully empty.
- Thin strip is hard to scan; does not read as “blocks.”
- Legend is words only — weak color mapping and accessibility.
- No summary %, no file boundaries, no per-bucket peek.

---

## 3. Target UI (ASCII)

### 3.1 Full section (downloading)

```
Transfer
┌──────────┐ ┌──────────┐ ┌──────────┐
│ Download │ │ Upload   │ │ ETA      │
│ 2.1 MB/s │ │ 180 KB/s │ │ 12m      │
└──────────┘ └──────────┘ └──────────┘
12 peers

Connection details · 2/4 trackers responding          Show

Pieces · 66% verified · 41 receiving           1,248 · 256 KB
┌────────────────────────────────────────────────────────────┐
│                                                            │
│  ■ ■ ■ ■ ■ ■ ■ ▧ ▧ ▧ ░ ░ ░ ░ ░ ░                          │
│  ░ ░ ░ ░ ■ ■ ■ ■ ■ ■ ■ ■ ▧ ░ ░ ░                          │
│  ░ ░ ░ ░ ░ ░ ░ ░ ○ ○ ○ ○ ○ ○ ○ ○                          │
│  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○                          │
│  └─ file A ─┘ └── file B ──┘ └──── file C (excl.) ────┘   │
│                                                            │
│  ■ Verified 820      ▧ Mixed / receiving 41                │
│  ░ Missing 312       ○ Excluded 75                         │
│                                                            │
│  Bucket 7 · pieces 56–63 · 3 verified · 2 receiving        │  ← after tap (optional)
│                                                            │
└────────────────────────────────────────────────────────────┘
```

### 3.2 Cell legend (glyphs for mocks)

| Glyph | State | Visual treatment |
|-------|--------|------------------|
| `■` | Verified | Solid fill — `colorScheme.primary` |
| `▧` | Mixed / receiving | Split fill inside one soft tile (see §4) |
| `░` | Missing | Dim fill — `surfaceVariant` |
| `○` | Excluded | Hollow / outline — `outlineVariant` |

### 3.3 Mixed-fill close-up

One bucket that is not pure gets a **split** inside the same rounded rect (vertical preferred; diagonal OK):

```
  ┌─────┐
  │██░░░│   left  = verified portion
  └─────┘   right = missing and/or receiving portion
```

Or stacked:

```
  ┌─────┐
  │█████│   top    = verified
  │▒▒▒▒▒│   middle = receiving (if any)
  │░░░░░│   bottom = missing
  └─────┘
```

**Rule:** proportions should reflect `verified` / `receiving` / remaining selected missing within the bucket. Excluded-only buckets stay hollow (`○`), not split.

### 3.4 Tiny torrent (larger cells)

```
Pieces · 50% verified · 1 receiving              12 · 4 MB each
┌────────────────────────────┐
│  ■ ■ ■ ▧ ░ ░               │
│  ░ ░ ○ ○ ○ ○               │
│  └ A ┘ └── B (excl.) ──┘   │
│                            │
│  ■ 3   ▧ 1   ░ 4   ○ 4     │
└────────────────────────────┘
```

### 3.5 Complete / paused (static)

```
Pieces · 100% verified · 0 receiving             48 · 1 MB each
┌────────────────────────────────┐
│  ■ ■ ■ ■ ■ ■ ■ ■               │
│  ■ ■ ■ ■ ■ ■ ■ ■               │
│  ■ ■ ■ ■ ■ ■ ■ ■               │
│  ■ ■ ■ ■ ■ ■ ■ ■               │
│                                │
│  ■ Verified 48                 │
│  ░ Missing 0   ○ Excluded 0    │
└────────────────────────────────┘
```

No receiving pulse when `entry.paused || entry.complete` (same freeze rule as today).

### 3.6 Unavailable

Keep the same card chrome as Transfer / Pieces:

```
Pieces
┌────────────────────────────────────────────┐
│  Map unavailable                           │
│  Engine offline or telemetry not ready yet │
└────────────────────────────────────────────┘
```

---

## 4. State mapping (from `PieceBucket`)

Fields per bucket: `start`, `end` (inclusive zero-based), `total`, `selected`,
`verified`, `receiving`, plus additive **`selectedVerified`** and
**`selectedReceiving`**.

All-piece `verified` / `receiving` remain for diagnostics. Grid fills, legend
counts, and title percent use **selected-aware** fields only. `receiving` /
`selectedReceiving` exclude verified (complete wins). Selection is a separate
dimension from completion.

Four disjoint counts per bucket (sum to `total`):

| Category | Definition |
|----------|------------|
| Selected verified | `selectedVerified` |
| Selected receiving | `selectedReceiving` |
| Selected missing | `selected - selectedVerified - selectedReceiving` (clamped ≥ 0) |
| Excluded | `total - selected` |

### 4.1 Dominant cell kind (for icon / legend counting)

Apply in order:

1. **Excluded** — `selected == 0` → hollow tile (`○`), even if all-piece `verified` is high.
2. **Fully verified** — `selectedVerified == selected` and `selected > 0` and no excluded remainder → solid (`■`).
3. **Mixed / receiving** — `selectedReceiving > 0` **or** remaining missing/excluded inside the cell → split tile (`▧`).
4. **Missing** — else → dim fill (`░`).

### 4.2 Split fill fractions (mixed cells)

Fractions divide by **`total`** (never paint a mixed bucket as fully verified):

| Layer | Fraction of cell |
|-------|------------------|
| Verified | `selectedVerified / total` |
| Receiving | `selectedReceiving / total` |
| Missing | `selectedMissing / total` |
| Excluded | `excluded / total` (hollow/outline band) |

If `selected == 0`, do not split — draw excluded hollow.

### 4.3 Summary % in title

- **Verified %** = sum(`selectedVerified`) / sum(`selected`) over buckets (null → **No selected pieces** when selected is 0).
- **Receiving count** = sum(`selectedReceiving`).

Title pattern:

```
Pieces · {pct}% verified · {receiving} receiving
```

or `No selected pieces` when nothing is selected.

Trailing meta:

```
{totalPieces} · {formatBytes(pieceLength)}
```

---

## 5. Grid layout rules

### 5.1 Cap

Engine already returns at most **`maxBuckets` (256)**. UI must never invent more cells than `buckets.size`.

### 5.2 Columns / cell size

Adaptive wrap:

| Bucket count | Approx columns | Cell size (guide) |
|--------------|----------------|-------------------|
| ≤ 64 | 8 | ~12–14 dp |
| 65–128 | 12–16 | ~10–12 dp |
| 129–256 | 16 | ~8–10 dp |

- Corner radius: ~2–3 dp (soft tile, not circle).
- Gap: ~1.5–2 dp between cells.
- Grid width: `fillMaxWidth` inside padded card (~14 dp padding to match Transfer cards).

### 5.3 Row math

```
columns = chosen from table above
rows    = ceil(bucketCount / columns)
```

Draw left → right, top → bottom in torrent piece order (bucket index order). Full torrent order left-to-right / wrap — same as UX spec.

### 5.4 File-boundary ticks (recommended v1)

If file list + piece length are available:

- Map each selected (or all) file’s byte range → piece index → bucket index.
- Draw a short tick or label under the first bucket that starts a file.
- Multi-file: show 2–3 labels max on small screens; rest as ticks only to avoid clutter.
- Shared boundary pieces: tick once; do not imply byte-exact isolation (per UX spec).

ASCII:

```
■ ■ ■ ■ ▧ ░ ░ ░ ■ ■ ■ ■ ○ ○ ○ ○
└file A┘ └─ file B ─┘ └─ C ─┘
```

---

## 6. Legend

Chip legend with **selected-aware disjoint piece counts**:

| Chip | Count source |
|------|----------------|
| Verified | sum of `selectedVerified` |
| Mixed / receiving | sum of `selectedReceiving` |
| Missing | sum of selected missing |
| Excluded | sum of `(total - selected)` |

Use color **plus** label (accessibility). Patterns (hatch) are optional v1.1 if contrast fails.

---

## 7. Interaction

### 7.1 Tap / long-press peek (recommended full version)

On cell tap, show one line under the legend (no dialog):

```
Bucket {i} · pieces {start}–{end} · {verified} verified · {receiving} receiving · {missing} missing
```

Use inclusive `PieceBucket.start` / `end`. Clear peek when tapping outside the grid (legend/padding).

### 7.2 Filter chip (nice polish)

```
[ All pieces ]  [ Selected only ]
```

“Selected only” hides or collapses excluded rows so multi-file deselection does not dominate the grid. Default: All pieces (matches current map).

### 7.3 No required gesture for v1 minimum

Minimum ship can omit tap-peek and filter; keep static grid + legend + summary.

---

## 8. Motion and lifecycle

| Condition | Behavior |
|-----------|----------|
| Details visible + foreground | Poll pieces ≤ 1 Hz (existing) |
| Paused | Freeze receiving appearance; no pulse |
| Complete | Stable verified map; no pulse |
| Leaving Details / background | Stop polling (existing) |
| Telemetry null / empty | “Map unavailable” card body |

Optional: subtle opacity breathe on cells that currently have `receiving > 0`. Keep amplitude low.

---

## 9. Placement on Details

Keep section order from UX spec:

1. Cover / title / status controls  
2. Transfer (speeds, ETA, peers)  
3. Connection details (expandable trackers)  
4. **Pieces** ← this design  
5. Selected files  
6. Technical  

Do **not** merge Pieces into Transfer for this design (heat grid needs vertical space). A future compact “ribbon under Transfer” is a different option and out of scope here.

---

## 10. Implementation notes (when coding)

**Files**

- `PieceHeatGrid.kt` / `PieceHeatGridModel.kt` — section UI + pure normalize/layout/hit-test/boundaries.
- `TorrentFilesScreen.kt` — wires `PieceHeatGridSection`; Details-lifecycle `pieces` poll ≤ 1 Hz via `SystemClock.elapsedRealtime`; clears map on engine-id change; first fetch prompt; stops off-screen.
- `engine-go` + `EngineClient` — additive `selectedVerified` / `selectedReceiving`.
- Prefer **one `Canvas`** capped at 256 cells with rounded clip for mixed stacks.
- Colors: `MaterialTheme.colorScheme` — `primary`, `tertiary` (receiving), `surfaceVariant` (missing), `outlineVariant` (excluded).
- Tests: `PieceHeatGridModelTest`, `EngineClientTest` selected-count parse, Go piece telemetry regressions.

**Do not**

- Request more than 256 buckets.
- Treat buckets as exact network blocks (label remains **Pieces**; each tile is a contiguous piece range).
- Invent seed counts or availability.

---

## 11. Ship phases

### v1 minimum (shipped)

- [x] Soft-tile wrapping heat grid  
- [x] Mixed-fill for non-pure buckets (selected-aware fractions / total)  
- [x] Chip legend with selected-aware counts  
- [x] Title summary: selected `% verified` · `selectedReceiving` (or No selected pieces)  
- [x] Meta: total pieces · piece length  
- [x] Static map when paused/complete (`frozen`; no pulse in v1)  
- [x] Map unavailable empty state in card  

### v1 recommended extras (shipped)

- [x] File-boundary ticks / short labels (max 2–3; shared wording)  
- [x] Tap cell → peek line (inclusive start–end)  

### Deferred polish (v1.1+)

- [ ] Selected-only filter chip  
- [ ] Subtle receiving pulse (only when not frozen)  
- [ ] Pattern/hatch for a11y if color contrast fails  
- [ ] “All selected pieces verified” completion line  

### Out of scope

- Beveled 3D cells  
- Pure opacity heatmap as sole encoding  
- Per-cell animation of all tiles  
- Raw bitfield (thousands of cells)  
- Circle/dot grid as primary metaphor (rejected in favor of soft tiles)

---

## 12. Style alternatives considered (for history)

| Style | Verdict |
|-------|---------|
| Soft tiles + mixed-fill | **Chosen** |
| Soft tiles only (no split) | Weaker honesty |
| Hard pixel squares | More “classic,” harsher on M3 |
| Circle/dot grid | Softer, weaker “blocks” |
| Density opacity heatmap alone | Pretty, poor state clarity |
| Pattern + color | Good a11y add-on, not required first |
| Glow receiving | OK as light pulse on top of chosen style |
| Beveled chips | Skip |
| Strip under Transfer (Option B) | Different direction; not this doc |
| Summary-only / map on expand | Optional later; not default |

---

## 13. Acceptance checklist (design → QA)

Automated / code-level:

- [x] Mixed bucket never renders as fully verified (unit + canvas fractions).  
- [x] Paused/complete: static (`frozen`); v1 has no receiving pulse either way.  
- [x] Deselected / excluded-only cells stay hollow; shared boundary wording supported.  
- [x] Cap ≤ 256 cells; adaptive columns for small/large maps.  
- [x] Legend uses color + label + selected-aware counts.  
- [x] Null/empty telemetry shows Map unavailable without breaking Details.  
- [x] Details lifecycle poll ≤ 1 Hz; clears on engine-id change; stops off-screen.  

Manual device checks (not claimed unless performed):

- [ ] Controlled torrent with known completion matches bucket paints.  
- [ ] Out-of-order completion visible as scattered `■` / `▧`.  
- [ ] Live download map updates while Details stays open.

---

## 14. Quick reference ASCII (paste into mock reviews)

```
Pieces · 66% verified · 41 receiving           1,248 · 256 KB
┌────────────────────────────────────────────────────────────┐
│  ■ ■ ■ ■ ■ ■ ▧ ▧ ░ ░ ░ ░ ■ ■ ■ ■                          │
│  ■ ■ ■ ■ ░ ░ ░ ░ ○ ○ ○ ○ ○ ○ ○ ○                          │
│  └─ A ─┘ └── B ──┘ └──── C (excluded) ────┘               │
│                                                            │
│  ■ Verified 820   ▧ Mixed/Receiving 41                     │
│  ░ Missing 312    ○ Excluded 75                            │
│                                                            │
│  Bucket 7 · pieces 56–63 · 3 verified · 2 receiving        │
└────────────────────────────────────────────────────────────┘

■ verified   ▧ mixed/receiving (split fill)   ░ missing   ○ excluded
```

---

*Saved for UI design handoff. Align any conflicts with `docs/DOWNLOAD_UX_SPEC.md` §5; this note specializes visualization only.*
