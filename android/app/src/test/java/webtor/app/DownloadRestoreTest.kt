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

    @Test
    fun parseLibrarySnapshotFiltersOutTombstones() {
        val entry1 = pending().copy(key = "key1", title = "Torrent 1")
        val entry2 = pending().copy(key = "key2", title = "Torrent 2")
        val encoded = DownloadStorage.encodeLibrary(listOf(entry1, entry2))

        val restored = parseLibrarySnapshot(encoded, setOf("key1"))
        assertEquals(1, restored.size)
        assertEquals("key2", restored[0].key)
    }

    @Test
    fun parseLibrarySnapshotPreservesValidEntriesWhenOneIsCorrupt() {
        val entry1 = pending().copy(key = "valid-key", title = "Valid Torrent")
        val validJson = org.json.JSONArray(DownloadStorage.encodeLibrary(listOf(entry1))).getJSONObject(0)
        // Corrupt entry has missing key / invalid structure
        val corruptJson = org.json.JSONObject("""{"badField":123}""")
        val array = org.json.JSONArray().put(validJson).put(corruptJson)

        val restored = parseLibrarySnapshot(array.toString(), emptySet())
        assertEquals(1, restored.size)
        assertEquals("valid-key", restored[0].key)
    }

    @Test
    fun explicitReAddClearsTombstoneAndSurvivesRestart() {
        val tempDir = java.nio.file.Files.createTempDirectory("storage-test-tomb").toFile()
        try {
            val (storage, _) = createTestStorage(tempDir)
            val e1 = pending().copy(key = "0123456789012345678901234567890123456789", title = "Torrent 1")

            // Retire entry
            storage.retireTorrent(e1.key, generation = 1L)
            assertTrue(e1.key in storage.loadTombstones())

            // Explicitly re-add/save entry
            storage.save(listOf(e1))
            assertFalse(e1.key in storage.loadTombstones())

            // On reload, entry is restored and not filtered out
            val loaded = storage.load()
            assertEquals(1, loaded.size)
            assertEquals(e1.key, loaded[0].key)
        } finally {
            tempDir.deleteRecursively()
        }
    }

    @Test
    fun fallbackTriesNewestValidSnapshotWhenNewestSnapshotIsCorrupt() {
        val tempDir = java.nio.file.Files.createTempDirectory("storage-test-snapshot").toFile()
        try {
            val (storage, prefs) = createTestStorage(tempDir)
            val e1 = pending().copy(key = "0123456789012345678901234567890123456789", title = "Valid Snapshot Torrent")

            // Save valid snapshot
            storage.save(listOf(e1))

            // Create a newer corrupt snapshot
            val snapshotsDir = java.io.File(tempDir, "files/snapshots")
            val corruptSnapshot = java.io.File(snapshotsDir, "snapshot-${System.currentTimeMillis() + 1000}.json")
            corruptSnapshot.writeText("{corrupt: json syntax error [")

            // Corrupt primary library
            prefs.edit().putString("library", "not-a-json-array").commit()

            // Load should skip corrupt snapshot and successfully recover from earlier valid snapshot
            val loaded = storage.load()
            assertEquals(1, loaded.size)
            assertEquals(e1.key, loaded[0].key)
            assertTrue(storage.loadWarning?.contains("Recovered library from backup snapshot") == true)
        } finally {
            tempDir.deleteRecursively()
        }
    }

    @Test
    fun damagedIndividualRecordIsRecoveredFromPerTorrentMetadata() {
        val tempDir = java.nio.file.Files.createTempDirectory("storage-test-meta").toFile()
        try {
            val (storage, prefs) = createTestStorage(tempDir)
            val e1 = pending().copy(key = "1111111111111111111111111111111111111111", title = "Torrent One", metadata = "d8:announce...e")
            val e2 = pending().copy(key = "2222222222222222222222222222222222222222", title = "Torrent Two", metadata = "d8:announce...e")

            storage.save(listOf(e1, e2))

            // Corrupt entry 1 in prefs library while leaving entry 2 valid
            val array = org.json.JSONArray(prefs.getString("library", "[]"))
            val obj1 = array.getJSONObject(0)
            // Break file list schema or required fields in obj1 so parseEntry fails
            obj1.put("files", "not-an-array")
            prefs.edit().putString("library", array.toString()).commit()

            val loaded = storage.load()
            assertEquals(2, loaded.size)
            assertTrue(loaded.any { it.key == e1.key })
            assertTrue(loaded.any { it.key == e2.key })
            val recoveredE1 = loaded.first { it.key == e1.key }
            assertEquals("Torrent One", recoveredE1.title)
            assertTrue(recoveredE1.error?.contains("Recovered metadata") == true)
        } finally {
            tempDir.deleteRecursively()
        }
    }

    private fun createTestStorage(baseDir: java.io.File): Pair<DownloadStorage, FakeSharedPreferences> {
        val filesDir = java.io.File(baseDir, "files").apply { mkdirs() }
        val cacheDir = java.io.File(baseDir, "cache").apply { mkdirs() }
        val prefs = FakeSharedPreferences()
        val context = object : android.content.ContextWrapper(null) {
            override fun getFilesDir(): java.io.File = filesDir
            override fun getCacheDir(): java.io.File = cacheDir
            override fun getSharedPreferences(name: String?, mode: Int): android.content.SharedPreferences = prefs
        }
        return DownloadStorage(context) to prefs
    }

    private class FakeSharedPreferences : android.content.SharedPreferences {
        private val data = mutableMapOf<String, Any?>()

        override fun getAll(): MutableMap<String, *> = HashMap(data)
        override fun getString(key: String?, defValue: String?): String? = data[key] as? String ?: defValue
        override fun getStringSet(key: String?, defValues: MutableSet<String>?): MutableSet<String>? =
            @Suppress("UNCHECKED_CAST") (data[key] as? MutableSet<String>) ?: defValues
        override fun getInt(key: String?, defValue: Int): Int = data[key] as? Int ?: defValue
        override fun getLong(key: String?, defValue: Long): Long = data[key] as? Long ?: defValue
        override fun getFloat(key: String?, defValue: Float): Float = data[key] as? Float ?: defValue
        override fun getBoolean(key: String?, defValue: Boolean): Boolean = data[key] as? Boolean ?: defValue
        override fun contains(key: String?): Boolean = data.containsKey(key)
        override fun edit(): android.content.SharedPreferences.Editor = EditorImpl(this)
        override fun registerOnSharedPreferenceChangeListener(listener: android.content.SharedPreferences.OnSharedPreferenceChangeListener?) {}
        override fun unregisterOnSharedPreferenceChangeListener(listener: android.content.SharedPreferences.OnSharedPreferenceChangeListener?) {}

        private class EditorImpl(private val prefs: FakeSharedPreferences) : android.content.SharedPreferences.Editor {
            private val changes = mutableMapOf<String, Any?>()
            private var clear = false

            override fun putString(key: String?, value: String?): android.content.SharedPreferences.Editor = apply { key?.let { changes[it] = value } }
            override fun putStringSet(key: String?, values: MutableSet<String>?): android.content.SharedPreferences.Editor = apply { key?.let { changes[it] = values } }
            override fun putInt(key: String?, value: Int): android.content.SharedPreferences.Editor = apply { key?.let { changes[it] = value } }
            override fun putLong(key: String?, value: Long): android.content.SharedPreferences.Editor = apply { key?.let { changes[it] = value } }
            override fun putFloat(key: String?, value: Float): android.content.SharedPreferences.Editor = apply { key?.let { changes[it] = value } }
            override fun putBoolean(key: String?, value: Boolean): android.content.SharedPreferences.Editor = apply { key?.let { changes[it] = value } }
            override fun remove(key: String?): android.content.SharedPreferences.Editor = apply { key?.let { changes[it] = this } }
            override fun clear(): android.content.SharedPreferences.Editor = apply { clear = true }
            override fun commit(): Boolean {
                if (clear) prefs.data.clear()
                for ((k, v) in changes) {
                    if (v === this) prefs.data.remove(k) else prefs.data[k] = v
                }
                return true
            }
            override fun apply() { commit() }
        }
    }
}
