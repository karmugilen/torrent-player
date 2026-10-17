# Local redesign tickets

Agreement: [REDESIGN_SPEC.md](REDESIGN_SPEC.md). Baseline: `a226754`.
All delivery is local; no GitHub push or remote tickets.

| Ticket | State | Dependencies | Deliverable |
| --- | --- | --- | --- |
| UI-01 | Ready | — | Shared Aero, Antiquity and Retro design system; saved theme/effects preferences. |
| UI-02 | Ready | UI-01 | Watch-first library, Continue watching, filters/sort, safe batch controls and useful empty states. |
| UI-03 | Ready | UI-01 | Friendlier Add, episode/file browsing, and playback-first details with themed controls. |
| UI-04 | Ready | UI-01 | Replace circular reactor with expandable branching peer view; keep world map. |
| MEDIA-01 | Ready | — | Automatic persistent thumbnails, decoder fallbacks and truthful preview states. |
| MEDIA-02 | Ready | MEDIA-01 | Bounded verified preview reads for Direct Play/incomplete media without external-player side effects. |
| QA-01 | Ready | UI-01–04, MEDIA-01–02 | Focused regressions, existing suites, local APK and visual verification where available. |
| STORAGE-01 | Backlog | First redesign | Non-destructive moved-file/folder-access repair and clearer storage recovery. |

## UI-01 acceptance

- Three visually distinct selectable themes with Aero as default; selections
  survive restart. Shared functionality across themes.
- Glass/gloss, parchment/botanical/celestial ornament and classic bevels are real
  treatments, not only palette changes. Text remains readable.
- Reduced effects persists and suppresses nonessential motion.

## UI-02 acceptance

- Library exposes All/Active/Ready/Needs attention, stable sorting and search.
- Continue watching opens the bookmarked episode in an external player.
- Multi-selection uses stable keys and handles entries disappearing or failing
  individually. Destructive actions retain explicit keep-files/delete-files choice.
- A search/filter with no matches does not claim the library is empty.

## UI-03 acceptance

- Playable files appear before connection/tracker/piece diagnostics.
- Search, natural episode order and video/subtitle selection work with existing
  file-selection persistence. No embedded video player.
- Add, Direct Play, pause/resume and deletion retain existing intent and error paths.

## UI-04 acceptance

- Geographic map remains available. No circular reactor in the alternative view.
- Device branches into protocol groups and rectangular peer tiles; stable order,
  expandable groups, real activity, useful empty/paused/unavailable states.
- No fabricated latency, seeder status, throughput or country data.

## MEDIA-01 acceptance

- Automatic attempts on both Wi-Fi and mobile; no manual thumbnail button.
- Successful incomplete/Direct Play images survive process restart.
- Decode failures do not skip independent fallbacks or permanently poison cache.
- Cached thumbnail remains usable while paused/offline. Failed attempts are bounded.

## MEDIA-02 acceptance

- Real verified torrent reads can supply a thumbnail without saving a full video.
- Preview demand is bounded and released on success, timeout, pause or deletion.
- Does not mutate playback focus/history/resume hints or change peer algorithms.
- A preview cannot fabricate verification, revive stopped transfers, or continue
  fetching indefinitely when decoding fails. Exercise piece overlap and cancellation.

## QA-01 acceptance

- Record actual commands/results and any unavailable device checks below.
- Produce a distinct local debug APK and checksum; keep source work in local commits.

## Execution log

- Planning: current app already committed at `a226754`; working tree was clean.
- Agreed scope recorded. Broader adaptive/evolutionary engine work explicitly excluded.
