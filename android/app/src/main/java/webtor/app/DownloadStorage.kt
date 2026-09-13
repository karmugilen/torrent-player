package webtor.app

import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.os.StatFs
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.system.Os
import android.system.OsConstants
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import webtor.core.TorrentStatus

data class SavedFile(
    val index: Int, val name: String, val path: String, val length: Long,
    val uri: String? = null, val progress: Double = 0.0,
    val relativePath: String? = null,
)

enum class EntryLifecycleState {
    PREPARING,
    DOWNLOADING,
    PAUSING,
    PAUSED,
    STOPPING,
    STOPPED,
    COMPLETED,
    DELETING,
    ERROR;
}

data class DownloadEntry(
    val key: String, val title: String, val source: String, val metadata: String,
    val engineId: String?, val destination: String, val files: List<SavedFile>,
    val selected: Set<Int>, val paused: Boolean = false,
    val status: TorrentStatus? = null, val error: String? = null,
    val addedAt: Long = System.currentTimeMillis(),
    val generation: Long = 0L,
    val isDeleting: Boolean = false,
    val lifecycleState: EntryLifecycleState = if (paused) EntryLifecycleState.PAUSED else if (engineId != null) EntryLifecycleState.DOWNLOADING else EntryLifecycleState.STOPPED,
) {
    val total get() = files.filter { it.index in selected }.sumOf { it.length }
    val downloaded get() = files.filter { it.index in selected }.sumOf { (it.length * it.progress.coerceIn(0.0, 1.0)).toLong() }
    val progress get() = if (total == 0L) 1f else (downloaded.toDouble() / total).toFloat().coerceIn(0f, 1f)
    val complete get() = files.filter { it.index in selected }.all { it.length == 0L || it.progress >= 1.0 }

    fun controlsBusy(): Boolean = isDeleting || lifecycleState == EntryLifecycleState.PREPARING ||
        lifecycleState == EntryLifecycleState.PAUSING || lifecycleState == EntryLifecycleState.STOPPING ||
        lifecycleState == EntryLifecycleState.DELETING

    fun stateLabel(): String = when {
        isDeleting || lifecycleState == EntryLifecycleState.DELETING -> "Removing…"
        lifecycleState == EntryLifecycleState.PAUSING -> "Pausing…"
        lifecycleState == EntryLifecycleState.STOPPING -> "Stopping…"
        lifecycleState == EntryLifecycleState.PREPARING -> "Preparing…"
        error != null || lifecycleState == EntryLifecycleState.ERROR -> "Needs attention"
        complete || lifecycleState == EntryLifecycleState.COMPLETED -> "Complete"
        paused || engineId == null || lifecycleState == EntryLifecycleState.PAUSED ||
            lifecycleState == EntryLifecycleState.STOPPED -> "Paused"
        else -> "Downloading"
    }
}

fun shouldSkipStartupRestore(entry: DownloadEntry): Boolean =
    entry.complete || entry.paused || entry.isDeleting

fun restoreStillApplies(
    liveGeneration: Long,
    commandGeneration: Long,
    isDeleting: Boolean,
    shutdownInProgress: Boolean,
): Boolean = !isDeleting && !shutdownInProgress && liveGeneration == commandGeneration

class DownloadStorage(private val context: Context) {
    private val prefs = context.getSharedPreferences("downloads", Context.MODE_PRIVATE)
    private val resolver = context.contentResolver
    var folder: String?
        get() = prefs.getString("folder", null)
        set(value) { prefs.edit().putString("folder", value).commit() }
    var folderName: String
        get() = prefs.getString("folderName", "Chosen folder") ?: "Chosen folder"
        set(value) { prefs.edit().putString("folderName", value).commit() }
    var loadWarning: String? = null
        private set
    val freeBytes get() = runCatching {
        StatFs(Environment.getExternalStorageDirectory().absolutePath).availableBytes
    }.getOrDefault(-1L)
    private val legacyDir get() = File(context.filesDir, "webtorrent")
    val legacyBytes get() = legacyDir.walkTopDown().filter { it.isFile }.sumOf {
        runCatching { Os.stat(it.absolutePath).st_blocks * 512L }.getOrDefault(it.length())
    }
    val cacheBytes get() = context.cacheDir.takeIf { it.exists() }?.walkTopDown()?.filter { it.isFile }?.sumOf { it.length() } ?: 0L

    fun persistFolder(uri: Uri, displayName: String) {
        check(DocumentsContract.isTreeUri(uri)) { "Choose a folder to store downloads" }
        try {
            resolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
        } catch (t: Exception) {
            throw IllegalStateException("Cannot keep access to this folder. Choose a writable folder again.", t)
        }
        val name = displayName.trim().ifEmpty { "Chosen folder" }
        check(prefs.edit().putString("folder", uri.toString()).putString("folderName", name).commit()) {
            "Could not remember this folder"
        }
    }

    fun folderPermissionOk(): Boolean {
        val current = folder ?: return true
        val uri = Uri.parse(current)
        return resolver.persistedUriPermissions.any { it.uri == uri && it.isWritePermission }
    }

    fun clearFolder() {
        prefs.edit().remove("folder").putString("folderName", "Chosen folder").commit()
    }

    fun managedBytes(entries: List<DownloadEntry>): Long = entries.sumOf { it.total }

    fun downloadedBytes(entries: List<DownloadEntry>): Long = entries.sumOf { it.downloaded }

    fun clearCache() {
        val failed = importTorrents().filter { !it.delete() && it.exists() }
        check(failed.isEmpty()) { "Some temporary torrent files could not be removed" }
    }

    fun clearLegacy() {
        check(legacyDir.deleteRecursively()) { "Some old downloads could not be removed" }
        legacyDir.mkdirs()
    }

    fun save(entries: List<DownloadEntry>) {
        check(prefs.edit().putString("library", encodeLibrary(entries)).commit()) { "Could not save download library" }
    }

    fun exportJson(): String = prefs.getString("library", "[]") ?: "[]"

    fun importJson(raw: String): List<DownloadEntry> {
        val array = try {
            JSONArray(raw)
        } catch (_: org.json.JSONException) {
            error("Could not import library. The backup is not valid JSON.")
        }
        return (0 until array.length()).map { i ->
            try {
                parseEntry(array.getJSONObject(i))
            } catch (t: Exception) {
                throw IllegalStateException("Could not import library: item ${i + 1} is invalid.", t)
            }
        }
    }

    fun load(): List<DownloadEntry> {
        loadWarning = null
        val raw = prefs.getString("library", "[]") ?: "[]"
        fun preserveDamagedLibrary() {
            check(prefs.edit().putString("libraryRecovery-${System.currentTimeMillis()}", raw).commit()) {
                "Could not preserve damaged library. Free some space and restart the app."
            }
            loadWarning = "Some saved downloads could not be restored. Their library records were preserved for recovery; files have not been deleted."
        }
        val array = try { JSONArray(raw) } catch (_: org.json.JSONException) {
            preserveDamagedLibrary()
            return emptyList()
        }
        var damaged = false
        val result = (0 until array.length()).mapNotNull { i ->
            try {
                parseEntry(array.getJSONObject(i))
            } catch (_: Exception) {
                damaged = true
                null
            }
        }
        if (damaged) preserveDamagedLibrary()
        return result
    }

    fun fileAccessible(file: SavedFile): Boolean {
        val uriString = file.uri ?: return false
        if (uriString.isBlank()) return false
        val uri = runCatching { Uri.parse(uriString) }.getOrNull() ?: return false
        runCatching {
            resolver.query(uri, arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) return true
            }
        }
        return runCatching { resolver.openFileDescriptor(uri, "r")?.use { true } == true }.getOrDefault(false)
    }

    fun createFiles(entry: DownloadEntry, tree: String?): List<SavedFile> {
        val created = mutableListOf<Uri>()
        try {
            check(entry.selected.isNotEmpty()) { "Select at least one file to download" }
            check(entry.selected.all { selected -> entry.files.any { it.index == selected } }) { "Invalid file selection" }
            // A unique directory prevents collisions across repeated downloads.
            val group = "${safe(entry.title).take(70)}-${java.util.UUID.randomUUID().toString().take(12)}"
            val directories = mutableMapOf<String, Uri>()
            if (tree != null) {
                val uri = Uri.parse(tree)
                val root = DocumentsContract.buildDocumentUriUsingTree(uri, DocumentsContract.getTreeDocumentId(uri))
                directories[""] = DocumentsContract.createDocument(resolver, root, DocumentsContract.Document.MIME_TYPE_DIR, group)
                    ?: error("Cannot create download folder")
                created += directories.getValue("")
            } else check(Build.VERSION.SDK_INT >= 29) { "Choose a folder on this Android version" }
            return entry.files.map { file ->
                if (file.index !in entry.selected) return@map file
                val parts = file.path.split('/').filter { it.isNotEmpty() }.map(::safe).ifEmpty { listOf(safe(file.name)) }
                val relativePath = "${Environment.DIRECTORY_DOWNLOADS}/Webtor/$group/" +
                    parts.dropLast(1).joinToString("/")
                val uri = if (tree != null) {
                    var parent = directories.getValue("")
                    var key = ""
                    for (part in parts.dropLast(1)) {
                        key += "/$part"
                        parent = directories.getOrPut(key) {
                            DocumentsContract.createDocument(resolver, parent, DocumentsContract.Document.MIME_TYPE_DIR, part)
                                ?: error("Cannot create subfolder")
                        }.also { if (it !in created) created += it
                        }
                    }
                    DocumentsContract.createDocument(resolver, parent, mime(file.name), parts.last())
                        ?: error("Cannot create ${file.name}")
                } else {
                    val values = ContentValues().apply {
                        put(MediaStore.MediaColumns.DISPLAY_NAME, parts.last())
                        put(MediaStore.MediaColumns.MIME_TYPE, mime(file.name))
                        put(MediaStore.MediaColumns.RELATIVE_PATH, relativePath)
                    }
                    if (Build.VERSION.SDK_INT < 29) error("Choose a folder on this Android version")
                    resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                        ?: error("Cannot create file in Downloads")
                }
                created += uri
                file.copy(uri = uri.toString(), relativePath = if (tree == null) relativePath else null)
            }
        } catch (t: Throwable) {
            created.asReversed().forEach { runCatching { deleteUri(it) } }
            throw t
        }
    }

    // Keep originals alive until Node has duplicated them. Node then owns its
    // copies; these Kotlin descriptors can be closed immediately after configure.
    fun openFiles(files: List<SavedFile>): List<ParcelFileDescriptor?> {
        val handles = mutableListOf<ParcelFileDescriptor?>()
        try {
            files.forEach { f ->
                val pfd = f.uri?.let { resolver.openFileDescriptor(Uri.parse(it), "rw") ?: error("File is unavailable: ${f.name}") }
                handles += pfd
                if (pfd != null) Os.lseek(pfd.fileDescriptor, 0, OsConstants.SEEK_SET)
            }
            return handles
        } catch (t: Throwable) {
            handles.forEach { runCatching { it?.close() } }
            throw IllegalStateException("Cannot access this folder. Choose a writable local folder. ${t.message}", t)
        }
    }

    fun deleteFiles(files: List<SavedFile>) {
        // Query before deleting so downloads saved by older versions are covered too.
        val paths = files.mapNotNull { file ->
            file.relativePath ?: file.uri?.let { value ->
                val uri = Uri.parse(value)
                if (Build.VERSION.SDK_INT < 29 || uri.authority != MediaStore.AUTHORITY) null
                else resolver.query(uri, arrayOf(MediaStore.MediaColumns.RELATIVE_PATH), null, null, null)
                    ?.use { if (it.moveToFirst()) it.getString(0) else null }
            }
        }
        val targetUris = files.mapNotNull { it.uri }.distinct()
        val failed = targetUris.filter { uriString ->
            runCatching { deleteUri(Uri.parse(uriString)) }.isFailure
        }
        check(failed.isEmpty()) { "Could not delete ${failed.size} file(s). Check folder access and retry." }
        val root = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "Webtor")
        val directories = paths.flatMap { relative ->
            downloadDirectories(root, relative)
        }.distinct().sortedByDescending { it.path.length }
        for (directory in directories) {
            // rmdir only: never recursively erase a folder or another app's content.
            if (directory.exists() && directory.list()?.isEmpty() == true) {
                directory.delete()
            }
        }
    }

    private fun deleteUri(uri: Uri) {
        if (DocumentsContract.isDocumentUri(context, uri)) {
            try {
                check(DocumentsContract.deleteDocument(resolver, uri)) { "Delete failed" }
            } catch (_: java.io.FileNotFoundException) {
                /* Already removed in Files. */
            } catch (_: IllegalArgumentException) {
                /* Document does not exist. */
            }
        } else {
            check(uri.scheme == "content" && uri.authority == MediaStore.AUTHORITY) { "Unrecognized download location" }
            val deleted = resolver.delete(uri, null, null)
            if (deleted == 0) {
                // A zero count can mean either an already missing file or a refused deletion.
                val stillExists = runCatching {
                    resolver.query(uri, arrayOf(MediaStore.MediaColumns._ID), null, null, null)?.use { it.moveToFirst() }
                }.getOrNull() == true
                check(!stillExists) { "Provider refused to delete file" }
            }
        }
    }

    private fun encodeLibrary(entries: List<DownloadEntry>): String {
        val array = JSONArray()
        entries.forEach { e ->
            array.put(JSONObject().put("key", e.key).put("title", e.title).put("source", e.source)
                .put("metadata", e.metadata).put("engineId", e.engineId).put("destination", e.destination)
                .put("paused", e.paused).put("addedAt", e.addedAt)
                .put("generation", e.generation)
                .put("isDeleting", e.isDeleting)
                .put("lifecycleState", e.lifecycleState.name)
                .put("selected", JSONArray(e.selected.toList()))
                .put("files", JSONArray().apply {
                    e.files.forEach { f -> put(JSONObject().put("index", f.index).put("name", f.name)
                        .put("path", f.path).put("length", f.length).put("uri", f.uri).put("progress", f.progress)
                        .put("relativePath", f.relativePath)) }
                }))
        }
        return array.toString()
    }

    private fun parseEntry(e: JSONObject): DownloadEntry {
        val files = e.getJSONArray("files")
        val selected = e.getJSONArray("selected")
        val stateName = e.optString("lifecycleState", "")
        val savedState = EntryLifecycleState.entries.find { it.name == stateName }
        val isDeleting = e.optBoolean("isDeleting", false)
        val paused = e.optBoolean("paused", false)
        val lifecycleState = when {
            isDeleting -> EntryLifecycleState.ERROR
            savedState == EntryLifecycleState.PAUSING -> EntryLifecycleState.PAUSED
            savedState == EntryLifecycleState.STOPPING || savedState == EntryLifecycleState.PREPARING -> EntryLifecycleState.STOPPED
            savedState != null -> savedState
            paused -> EntryLifecycleState.PAUSED
            else -> EntryLifecycleState.STOPPED
        }
        val entry = DownloadEntry(
            e.getString("key"), e.getString("title"), e.getString("source"), e.getString("metadata"),
            if (e.isNull("engineId")) null else e.getString("engineId"), e.getString("destination"),
            (0 until files.length()).map { n -> files.getJSONObject(n).let { f ->
                SavedFile(f.getInt("index"), f.getString("name"), f.getString("path"), f.getLong("length"),
                    if (f.isNull("uri")) null else f.getString("uri"),
                    f.optDouble("progress", 0.0).let { if (it.isFinite()) it.coerceIn(0.0, 1.0) else 0.0 },
                    if (f.isNull("relativePath")) null else f.getString("relativePath"))
            } },
            (0 until selected.length()).map { selected.getInt(it) }.toSet(),
            paused || isDeleting || savedState == EntryLifecycleState.PAUSING || savedState == EntryLifecycleState.STOPPING,
            status = null,
            error = if (isDeleting) "Deletion was interrupted. Tap Remove to retry." else null,
            addedAt = e.optLong("addedAt", 0L),
            generation = e.optLong("generation", 0L),
            isDeleting = false,
            lifecycleState = lifecycleState,
        )
        require(entry.key.isNotBlank()) { "Missing torrent identity" }
        require(entry.files.isNotEmpty() && entry.files.map { it.index }.toSet().size == entry.files.size) { "Invalid torrent files" }
        require(entry.files.withIndex().all { (index, file) -> file.index == index && file.length >= 0 }) { "Invalid file index or length" }
        require(entry.selected.isNotEmpty() && entry.selected.all { it in entry.files.indices }) { "Invalid file selection" }
        return entry
    }

    private fun importTorrents(): List<File> = context.cacheDir.listFiles().orEmpty().filter {
        it.isFile && it.name.startsWith("import-") && it.name.endsWith(".torrent")
    }

    private fun safe(name: String): String = name.replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_")
        .trim().take(120).let { if (it.isBlank() || it == "." || it == "..") "download" else it }
    private fun mime(name: String) = android.webkit.MimeTypeMap.getSingleton()
        .getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "application/octet-stream"
}

// Only return ancestors within the app's download root; never the shared root itself.
internal fun downloadDirectories(root: File, relativePath: String): List<File> {
    val parts = relativePath.trim('/').split('/')
    if (parts.size < 3 || parts.take(2) != listOf("Download", "Webtor") ||
        parts.any { it == "." || it == ".." || it.contains('\\') }) return emptyList()
    val canonicalRoot = root.canonicalFile
    val leaf = File(root, parts.drop(2).joinToString("/")).canonicalFile
    if (!leaf.path.startsWith(canonicalRoot.path + File.separator)) return emptyList()
    return generateSequence(leaf) { it.parentFile }.takeWhile { it != canonicalRoot }.toList()
}
