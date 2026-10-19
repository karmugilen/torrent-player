# External-player HTTP experiment

Checkpoint before this experiment: `003365e` (Android 2.1.6).

Open **Settings → Playback method** and select **Standard** or **HTTP stream (experimental)**. The choice is saved across restarts and applies to Play for incomplete video and Direct Play. Standard is the initial default and uses the Android content provider. Video playback buttons always show Play and continue to pass saved positions to compatible players. The episode three-dot menu and separate play-from-start action have been removed. Completed downloaded files continue to open directly through their saved descriptors.

## What changes

| Path | Player receives | Read and seek behavior |
| --- | --- | --- |
| Default | A read-only `content://` URI | Android calls a seekable proxy descriptor; each handle dispatches callbacks on one handler thread. |
| Experiment | A `http://127.0.0.1:<port>/torrent/<id>/file/<index>` URL | The existing Go endpoint serves byte ranges; a disconnected request cancels its playback reader, and another request can seek independently. |

Both paths use the same verified torrent storage, piece priorities and RAM cache. Neither path supplies decoded frames. HTTP does not replace the torrent engine or make unavailable pieces appear sooner. The listener stays on loopback.

The Android proxy API requires the requested byte count unless it reaches EOF: [ProxyFileDescriptorCallback.onRead](https://developer.android.com/reference/android/os/ProxyFileDescriptorCallback#onRead(long,int,byte%5B%5D)). A missing-data read can therefore hold up later callbacks on the same descriptor's handler. HTTP's [Range request](https://www.rfc-editor.org/rfc/rfc9110.html#name-range) mechanism provides a second delivery interface, with request cancellation independent of that Android handler. A player still decides when it disconnects an obsolete request.

## Caching and lifetime

The HTTP endpoint previously sent `no-cache, no-store, must-revalidate`. It now sends `private, max-age=3600` for media responses, allowing private HTTP caches to retain verified bytes. This does not force a player to cache, set its buffer size, or create a new disk cache in Webtor. Player media caches are configured by the player itself; see Android's [media caching documentation](https://developer.android.com/media/media3/exoplayer/network-stacks#caching-media).

An HTTP launch holds a process-owned playback lease until the player returns, launch fails, or the user stops all transfers. This spans gaps between HTTP range requests and Activity recreation. The lease also keeps existing playback notifications and seeding-pause policy active. Sidecar subtitles retain content-URI grants. HTTP playlist URLs map back to canonical episode URIs before bookmarks are saved. An ambiguous playlist return does not overwrite an episode's bookmark.

## Validation and next comparison

Automated checks cover ordered HTTP playlist construction, subtitle and resume preservation, canonical bookmark mappings, loopback endpoint validation, and playback-lease replacement and cleanup. Engine checks cover HEAD, verified partial and complete ranges, suffix ranges, unsatisfiable ranges, and disconnecting a request waiting on missing data before successfully reading another range.

These are correctness checks. No Android player is connected in this environment, and no claim of improved seek speed or player cache behavior has been measured. Compare the two settings on the same episode, swarm and player. Record first-play delay, seek-to-picture delay, repeated backward seeks, player buffer settings, and behavior when returning from playback. A player may reject cleartext localhost URLs or use different network-buffer settings for HTTP.
