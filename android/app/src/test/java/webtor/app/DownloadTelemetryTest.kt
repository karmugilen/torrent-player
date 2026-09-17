package webtor.app

import org.junit.Assert.*
import org.junit.Test
import org.json.JSONObject
import webtor.core.EngineClient

class DownloadTelemetryTest {
    private fun active(id: String = "one", speed: Long = 100, peers: Int = 2) = DownloadEntry(
        key = id, title = id, source = "magnet:x", metadata = "", engineId = id,
        destination = "Downloads", files = listOf(SavedFile(0, "video.mp4", "video.mp4", 1000, progress = 0.2)),
        selected = setOf(0), lifecycleState = EntryLifecycleState.DOWNLOADING,
        status = EngineClient.parseTorrent(JSONObject("""{"id":"$id","ready":true,"downloadSpeed":$speed,"uploadSpeed":10,"numPeers":$peers}""")),
    )

    @Test fun differentTorrentsKeepIndependentRatesIncludingIdlePeers() {
        val first = active(speed = 100)
        val second = active("two", speed = 0, peers = 5)
        assertEquals("100 B/s · 2 peers · less than a minute", transferTelemetry(first))
        assertEquals("0 B/s · 5 peers", transferTelemetry(second))
        assertEquals("200 B of 1000 B · 0 B/s · 5 peers", entryTelemetry(second))
        assertEquals(8000L, first.selectedEta())
        assertNull(second.selectedEta())
    }

    @Test fun missingOrOldStatusIsUnknownRatherThanZeroOrHidden() {
        val missing = active().copy(status = null)
        assertEquals("—", transferTelemetry(missing))
        assertTrue(entryTelemetry(missing).endsWith(" · —"))
        assertNull(missing.liveDownloadSpeed())
        assertNull(missing.liveUploadSpeed())
        assertEquals("—", transferTelemetry(active().copy(engineId = "restored")))
        assertEquals("200 B / 1000 B · —", downloadNotification(listOf(missing))?.text)
    }

    @Test fun inactiveStatesDoNotShowStaleRatesOrEta() {
        val entry = active()
        for (inactive in listOf(
            entry.copy(paused = true), entry.copy(engineId = null),
            entry.copy(lifecycleState = EntryLifecycleState.PREPARING),
            entry.copy(lifecycleState = EntryLifecycleState.STOPPED),
            entry.copy(lifecycleState = EntryLifecycleState.PAUSED),
            entry.copy(files = entry.files.map { it.copy(progress = 1.0, verifiedBytes = it.length) }),
        )) {
            assertNull(transferTelemetry(inactive))
            assertNull(inactive.selectedEta())
            assertNull(downloadNotification(listOf(inactive)))
        }
    }

    @Test fun combinedNotificationUsesOneActiveSetForBytesSpeedPeersAndTitle() {
        val first = active()
        val second = active("two", speed = 50, peers = 3)
        val paused = active("paused", speed = 900, peers = 10).copy(paused = true)
        val notification = downloadNotification(listOf(first, second, paused))!!
        assertEquals("2 downloads", notification.title)
        assertEquals("400 B / 2.0 KB · 150 B/s · 5 peers", notification.text)
        assertEquals(20, notification.progress)
        assertTrue(notification.multiple)
        assertFalse(downloadNotification(listOf(first, paused))!!.multiple)
    }

    @Test fun waitingAndCheckingNotificationsAreHonest() {
        val entry = active()
        val waiting = active("waiting").copy(metadataReady = false, files = emptyList(), selected = emptySet())
        assertEquals("Waiting for peers", downloadNotification(listOf(waiting))?.text)
        assertEquals("200 B / 1000 B · 100 B/s · 2 peers · 1 waiting", downloadNotification(listOf(entry, waiting))?.text)
        val checking = entry.copy(status = entry.status!!.copy(checking = true, checkedPieces = 3, checkTotal = 10))
        assertEquals("Checking saved data · 30%", downloadNotification(listOf(checking))?.text)
        assertNull(transferTelemetry(checking))
        assertNull(checking.selectedEta())
        assertFalse(downloadNotification(listOf(entry, active("two").copy(status = null)))!!.text.contains("peers"))
    }
}
