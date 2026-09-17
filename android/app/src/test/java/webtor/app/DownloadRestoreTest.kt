package webtor.app

import org.junit.Assert.*
import org.junit.Test
import org.json.JSONObject
import webtor.core.EngineClient
import webtor.core.EngineResponse

class DownloadRestoreTest {
    private fun pending() = DownloadEntry(
        key = "0123456789012345678901234567890123456789", title = "Pending",
        source = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&tr=https%3A%2F%2Ftracker.example%2Fannounce",
        metadata = "", engineId = null, destination = "Downloads", files = emptyList(), selected = emptySet(),
        metadataReady = false, lifecycleState = EntryLifecycleState.PREPARING,
    )

    @Test fun coldPendingRestoreAddsPreparedMagnetAndAttachesPollableEngineId() {
        val saved = pending()
        assertTrue(shouldRestoreAtStartup(saved))
        assertNull(restoreValidationError(saved))
        val requests = mutableListOf<String>()
        val client = EngineClient { method, path, body ->
            requests += "$method $path"
            if (path == "/add") {
                val json = JSONObject(body!!)
                assertTrue(json.getBoolean("prepare"))
                assertEquals(saved.source, json.getString("torrentId"))
                assertFalse(json.has("torrentData"))
                EngineResponse(200, """{"id":"new-native-id"}""")
            } else EngineResponse(200, """{"id":"new-native-id","ready":false,"paused":false}""")
        }
        val added = client.addSavedDownload(saved)
        val restored = restoredPendingDiscovery(saved, added.id, client.torrent(added.id), false)
        assertEquals(listOf("POST /add", "GET /torrent/new-native-id"), requests)
        assertEquals("new-native-id", restored.engineId)
        assertFalse(restored.paused)
        assertFalse(restored.controlsBusy())
        assertEquals(saved.source, restored.source)
        assertEquals("Waiting for peers", restored.stateLabel())
        assertEquals("Waiting for peers", downloadSummaryLabel(restored))
        assertTrue(needsDiscoveryStatusRefresh(restored))
        assertFalse(shouldRestoreAtStartup(restored))
    }

    @Test fun restoreHonorsCurrentPauseAndDeletionAndNeverReusesOldStatus() {
        val saved = pending()
        assertFalse(shouldRestoreAtStartup(saved.copy(paused = true)))
        assertFalse(shouldRestoreAtStartup(saved.copy(isDeleting = true)))
        val stale = EngineClient.parseTorrent(JSONObject("""{"id":"old-native-id","downloadSpeed":9000}"""))
        val restored = restoredPendingDiscovery(saved.copy(status = stale), "new-native-id", null, false)
        assertNull(restored.status)
        assertTrue(needsDiscoveryStatusRefresh(restored))
        val paused = restoredPendingDiscovery(saved, "new-native-id", null, true)
        assertEquals("Paused", paused.stateLabel())
        assertFalse(needsDiscoveryStatusRefresh(paused))
        assertEquals("Needs attention", restored.copy(error = "Engine unavailable").stateLabel())
    }

    @Test fun pendingValidationRejectsMissingSourceButDoesNotRequireDestinations() {
        assertNull(restoreValidationError(pending()))
        assertNotNull(restoreValidationError(pending().copy(source = "")))
        val ready = pending().copy(metadataReady = true, files = listOf(SavedFile(0, "x", "x", 100)))
        assertNotNull(restoreValidationError(ready))
        assertFalse(shouldRestoreAtStartup(ready))
        assertTrue(shouldRestoreAtStartup(ready.copy(selected = setOf(0), files = ready.files.map { it.copy(uri = "content://downloads/1") })))
    }
}
