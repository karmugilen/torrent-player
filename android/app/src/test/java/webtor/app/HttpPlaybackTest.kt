package webtor.app

import org.junit.Assert.*
import org.junit.Test

class HttpPlaybackTest {
    private fun episode(index: Int, name: String) = SavedFile(index, name, name, 100, uri = "content://downloads/$index")
    private val first = episode(2, "Show.S01E02.mkv")
    private val second = episode(10, "Show.S01E10.mkv")
    private val subtitle = episode(3, "Show.S01E02.en.srt")
    private val entry = DownloadEntry(
        key = "series", title = "Show", source = "", metadata = "", destination = "Direct Play", engineId = "session",
        files = listOf(second, subtitle, first), selected = setOf(2, 3, 10), directPlay = true,
    )
    private val content = UiEvent.OpenContent(
        uri = "content://webtor.app.playback/series/2/video", mime = "video/x-matroska", name = first.name,
        playlist = listOf(PlaybackItem("content://webtor.app.playback/series/2/video", first.name), PlaybackItem("content://webtor.app.playback/series/10/video", second.name)),
        subtitles = listOf(PlaybackItem("content://webtor.app.playback/series/3/subtitle", subtitle.name)), positionMs = 45_000,
    )

    @Test fun httpLaunchRetainsEpisodeOrderResumeAndSubtitleGrants() {
        val http = httpPlaybackEvent(content, entry, first, "http://127.0.0.1:12345/torrent/session/file/2")
        assertEquals(listOf("http://127.0.0.1:12345/torrent/session/file/2", "http://127.0.0.1:12345/torrent/session/file/10"), http.playlist.map { it.uri })
        assertEquals(content.subtitles, http.subtitles)
        assertEquals(45_000L, http.positionMs)
        assertEquals(entry.key, http.playbackLeaseKey)
        assertEquals(content.uri, http.bookmarkUris[http.uri])
        assertEquals(content.playlist[1].uri, http.bookmarkUris[http.playlist[1].uri])
        assertEquals(http.playlist[1].uri, returnedPlaybackUri(http.playlist[1].uri, http.uri, http.playlist.map { it.uri }.toSet()))
        assertNull(returnedPlaybackUri(null, http.uri, http.playlist.map { it.uri }.toSet()))
    }

    @Test fun onlyMatchingLoopbackEndpointsAreAccepted() {
        for (url in listOf(
            "http://example.com:12345/torrent/session/file/2",
            "http://127.0.0.1:12345/torrent/other/file/2",
            "http://127.0.0.1:12345/torrent/session/file/10",
            "http://user@127.0.0.1:12345/torrent/session/file/2",
            "http://127.0.0.1:12345/torrent/session/file/2?extra=yes",
        )) {
            try {
                httpPlaybackEvent(content, entry, first, url)
                fail("Accepted $url")
            } catch (_: IllegalArgumentException) { }
        }
    }

    @Test fun httpLeaseSurvivesRequestGapsAndIgnoresOldPlayerReturns() {
        val leases = PlaybackLeases()
        val launch = HttpPlaybackLease(leases)
        launch.begin("http://127.0.0.1/one", "one")
        assertEquals(setOf("one"), leases.activeKeys())
        launch.begin("http://127.0.0.1/two", "two")
        launch.finish("http://127.0.0.1/one")
        assertEquals(setOf("two"), leases.activeKeys())
        launch.finish("http://127.0.0.1/two")
        launch.finish("http://127.0.0.1/two")
        assertTrue(leases.activeKeys().isEmpty())
        launch.begin("http://127.0.0.1/three", "three")
        launch.clear()
        assertTrue(leases.activeKeys().isEmpty())
    }
}
