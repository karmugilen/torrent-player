package webtor.app

import org.junit.Assert.*
import org.junit.Test

class PlaybackLeasesTest {
    @Test fun switchingVideosKeepsOtherOpenDescriptorsAlive() {
        val leases = PlaybackLeases()
        val first = leases.acquire("movie-one")
        val probe = leases.acquire("movie-one")
        val second = leases.acquire("movie-two")
        assertEquals(setOf("movie-one", "movie-two"), leases.activeKeys())
        first.close()
        first.close()
        assertEquals(setOf("movie-one", "movie-two"), leases.activeKeys())
        probe.close()
        assertEquals(setOf("movie-two"), leases.activeKeys())
        second.close()
        assertTrue(leases.activeKeys().isEmpty())
    }
}
