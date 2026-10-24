package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import webtor.core.NextPreload

class NextEpisodeWarmTest {
    private fun file(index: Int, name: String, length: Long = 100L, verified: Long = 0L) =
        SavedFile(index = index, name = name, path = "Show/$name", length = length, verifiedBytes = verified)

    private fun entry(vararg files: SavedFile, selected: Set<Int> = files.map { it.index }.toSet()) = DownloadEntry(
        key = "k", title = "Show", source = "magnet:?xt=urn:btih:abc", metadata = "meta",
        engineId = "e", destination = "", files = files.toList(), selected = selected,
    )

    @Test
    fun warmsTheNaturalNextEpisodeOnlyWithPlayNext() {
        val e2 = file(0, "Episode 2.mkv")
        val e10 = file(1, "Episode 10.mkv")
        val e1 = file(2, "Episode 1.mkv")
        val show = entry(e2, e10, e1, file(3, "notes.txt"))
        assertEquals(0, nextEpisodeToWarm(show, e1, playNext = true)?.index)
        assertEquals(1, nextEpisodeToWarm(show, e2, playNext = true)?.index)
        assertNull(nextEpisodeToWarm(show, e10, playNext = true))
        assertNull(nextEpisodeToWarm(show, e1, playNext = false))
        assertNull(nextEpisodeToWarm(show, file(3, "notes.txt"), playNext = true))
    }

    @Test
    fun skipsEpisodesThatNeedNoFetching() {
        val e1 = file(0, "Episode 1.mkv")
        assertNull(nextEpisodeToWarm(entry(e1, file(1, "Episode 2.mkv", verified = 100L)), e1, playNext = true))
        assertNull(nextEpisodeToWarm(entry(e1, file(1, "Episode 2.mkv"), selected = setOf(0)), e1, playNext = true))
    }

    @Test
    fun labelsOnlyTheUpcomingRow() {
        val planned = NextPreload(fileIndex = 1, active = false, ready = false)
        assertNull(upNextLabel(null, 1))
        assertNull(upNextLabel(planned, 0))
        assertEquals("Up next · loads near the end", upNextLabel(planned, 1))
        assertEquals("Up next · loading start", upNextLabel(planned.copy(active = true), 1))
        assertEquals("Up next · start loaded", upNextLabel(planned.copy(active = true, ready = true), 1))
    }
}
