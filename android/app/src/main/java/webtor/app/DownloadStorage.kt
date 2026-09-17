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
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import webtor.core.TorrentStatus

data class SavedFile(
    val index: Int, val name: String, val path: String, val length: Long,
    val uri: String? = null, val progress: Double = 0.0,
    val relativePath: String? = null,
    /** Hashed complete bytes. Display `progress` may include unverified receive. */
    val verifiedBytes: Long = 0L,
) {
    val isVerifiedComplete: Boolean get() = length == 0L || verifiedBytes >= length
}

data class CreatedDownloadFiles(
    val files: List<SavedFile>,
    val groupUri: String? = null,
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
    /** False while a magnet is saved but its metadata/file list is not known yet. */
    val metadataReady: Boolean = files.isNotEmpty() || metadata.isNotBlank(),
    /** Whether the selected set should be chosen automatically when metadata arrives. */
    val autoSelect: Boolean = true,
    /** Generation of the completion notification already acknowledged. */
    val completionAck: Long = 0L,
    /** SAF document URI for this download's generated directory. */
    val downloadGroupUri: String? = null,
    val focusedFile: Int? = null,
    val transferStage: TransferStage? = null,
    val transferReason: String? = null,
) {
    val total get() = files.filter { it.index in selected }.sumOf { it.length }
    val downloaded get() = files.filter { it.index in selected }.sumOf { (it.length * it.progress.coerceIn(0.0, 1.0)).toLong() }
    // An empty selection is an intentional stopped state, never a completed one.
    // A metadata-pending entry also has an unknown total and must not show 100%.
    val progress get() = if (!metadataReady || selected.isEmpty()) 0f else if (total == 0L) 1f else (downloaded.toDouble() / total).toFloat().coerceIn(0f, 1f)
    val checking get() = status?.checking == true
    val checkPercent get() = status?.let {
        if (it.checkTotal > 0) (100L * it.checkedPieces / it.checkTotal).toInt().coerceIn(0, 100) else 0
    } ?: 0
    val complete get() = metadataReady && selected.isNotEmpty() && !checking &&
        files.filter { it.index in selected }.all { it.isVerifiedComplete }

    fun controlsBusy(): Boolean = isDeleting || lifecycleState == EntryLifecycleState.PREPARING ||
        lifecycleState == EntryLifecycleState.PAUSING || lifecycleState == EntryLifecycleState.STOPPING ||
        lifecycleState == EntryLifecycleState.DELETING

    fun stateLabel(): String = when {
        isDeleting || lifecycleState == EntryLifecycleState.DELETING -> "Removing…"
        lifecycleState == EntryLifecycleState.PAUSING -> "Pausing…"
        lifecycleState == EntryLifecycleState.STOPPING -> "Stopping…"
        lifecycleState == EntryLifecycleState.PREPARING -> "Preparing…"
        paused || lifecycleState == EntryLifecycleState.PAUSED -> "Paused"
        !metadataReady -> "Waiting for peers"
        error != null || lifecycleState == EntryLifecycleState.ERROR -> "Needs attention"
        complete || lifecycleState == EntryLifecycleState.COMPLETED -> "Complete"
        transferReason != null && transferStage != TransferStage.DOWNLOADING -> transferReason
        paused || (engineId == null && lifecycleState != EntryLifecycleState.DOWNLOADING) ||
            lifecycleState == EntryLifecycleState.PAUSED ||
            lifecycleState == EntryLifecycleState.STOPPED -> "Paused"
        checking -> "Checking saved data"
        else -> "Downloading"
    }
}

fun shouldSkipStartupRestore(entry: DownloadEntry): Boolean =
    entry.complete || entry.paused || entry.isDeleting

fun requiresFileSelectionForResume(entry: DownloadEntry): Boolean =
    entry.metadataReady && entry.selected.isEmpty()

fun restoreValidationError(entry: DownloadEntry): String? {
    val hasAnyUri = entry.files.any { it.uri != null }
    if (hasAnyUri) {
        val hasFiles = entry.files.any { it.index in entry.selected && it.uri != null }
        if (entry.selected.isNotEmpty() && !hasFiles) {
            return "This download has no saved files to restore."
        }
    } else if (!entry.metadataReady && entry.files.isEmpty()) {
        // A metadata-pending magnet legitimately has no files or selected destinations yet.
        // Discovery must proceed without requiring file selection or storage checks.
    } else {
        if (entry.selected.isEmpty()) {
            return "Select at least one file to download."
        }
    }
    if (entry.metadata.isBlank() && entry.source.isBlank()) {
        return "This download has no torrent metadata to restore."
    }
    return null
}

fun downloadSummaryLabel(entry: DownloadEntry, error: String? = null): String = when {
    entry.controlsBusy() -> entry.stateLabel()
    error != null || entry.error != null -> "Needs attention"
    entry.complete -> "Complete"
    entry.paused || entry.lifecycleState == EntryLifecycleState.PAUSED ||
        (entry.metadataReady && (entry.lifecycleState == EntryLifecycleState.STOPPED || entry.engineId == null)) -> "Paused"
    else -> entry.stateLabel()
}

fun restoreStillApplies(
    liveGeneration: Long,
    commandGeneration: Long,
    isDeleting: Boolean,
    shutdownInProgress: Boolean,
): Boolean = !isDeleting && !shutdownInProgress && liveGeneration == commandGeneration

class DownloadStorage(private val context: Context) {
    fun selectedFolderName(): String = folderName
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

    fun managedBytes(entries: List<DownloadEntry>): Long = entries.sumOf { entry ->
        entry.files.filter { it.index in entry.selected && !it.uri.isNullOrBlank() }.sumOf { it.length }
    }

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

    fun prepareForPlayback(file: SavedFile) {
        val uri = Uri.parse(file.uri ?: error("Downloaded file is unavailable"))
        resolver.openFileDescriptor(uri, "r")?.use { fd ->
            check(fd.statSize < 0 || fd.statSize >= file.length) { "Downloaded file is shorter than expected. Resume it before playing." }
        } ?: error("Downloaded file is unavailable")
        if (Build.VERSION.SDK_INT >= 29 && uri.authority == MediaStore.AUTHORITY) {
            // Ask MediaStore to refresh metadata after native descriptor writes.
            // Older builds could leave an empty-file scan in its database.
            runCatching {
                resolver.update(uri, ContentValues().apply {
                    put(MediaStore.MediaColumns.IS_PENDING, 0)
                    put(MediaStore.MediaColumns.MIME_TYPE, mimeFor(file.name))
                }, null, null)
            }.onFailure { android.util.Log.w("webtor-playback", "Could not refresh saved-file metadata", it) }
        }
    }

    fun createFiles(entry: DownloadEntry, tree: String?): CreatedDownloadFiles {
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
            val files = entry.files.map { file ->
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
                    resolver.insert(mediaStoreDownloadsUri(), values)
                        ?: error("Cannot create file in Downloads")
                }
                created += uri
                file.copy(uri = uri.toString(), relativePath = if (tree == null) relativePath else null)
            }
            return CreatedDownloadFiles(files, directories[""]?.toString())
        } catch (t: Throwable) {
            created.asReversed().forEach { runCatching { deleteUri(it) } }
            throw t
        }
    }

    /**
     * Allocates one previously unselected file in the same MediaStore download
     * group as the entry's existing files. The file is created only after the
     * user selects it, so selecting a file later does not reserve all torrent
     * storage up front.
     */
    fun createAdditionalFile(entry: DownloadEntry, file: SavedFile): SavedFile {
        check(file.uri.isNullOrBlank()) { "File already has a saved destination" }
        val parts = file.path.split('/').filter { it.isNotEmpty() }.map(::safe)
            .ifEmpty { listOf(safe(file.name)) }
        val tree = folder
        if (tree != null) {
            val treeUri = Uri.parse(tree)
            // Reuse the exact generated group when available. Older records
            // have no group URI and fall back to the selected SAF root.
            val root = entry.downloadGroupUri?.takeIf { it.isNotBlank() }?.let(Uri::parse)
                ?: DocumentsContract.buildDocumentUriUsingTree(treeUri, DocumentsContract.getTreeDocumentId(treeUri))
            val created = mutableListOf<Uri>()
            return try {
                var parent = root
                for (part in parts.dropLast(1)) {
                    parent = DocumentsContract.createDocument(
                        resolver, parent, DocumentsContract.Document.MIME_TYPE_DIR, part,
                    ) ?: error("Cannot create subfolder")
                    created += parent
                }
                val uri = DocumentsContract.createDocument(resolver, parent, mime(file.name), parts.last())
                    ?: error("Cannot create ${file.name}")
                created += uri
                file.copy(uri = uri.toString())
            } catch (t: Throwable) {
                created.asReversed().forEach { runCatching { deleteUri(it) } }
                throw t
            }
        }
        check(Build.VERSION.SDK_INT >= 29) { "Choose a folder on this Android version" }
        val template = entry.files.asSequence().mapNotNull { it.relativePath }.firstOrNull { it.isNotBlank() }
        val parent = template?.substringBeforeLast('/', missingDelimiterValue = "Download/Webtor")
            ?.trimEnd('/')?.ifBlank { "Download/Webtor" } ?: "Download/Webtor"
        val relativePath = "$parent/${parts.dropLast(1).joinToString("/")}".trimEnd('/') + "/"
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, parts.last())
            put(MediaStore.MediaColumns.MIME_TYPE, mime(file.name))
            put(MediaStore.MediaColumns.RELATIVE_PATH, relativePath)
        }
        val uri = resolver.insert(mediaStoreDownloadsUri(), values)
            ?: error("Cannot create ${file.name}")
        return try {
            file.copy(uri = uri.toString(), relativePath = relativePath)
        } catch (t: Throwable) {
            runCatching { deleteUri(uri) }
            throw t
        }
    }

    // Keep originals alive until the Go engine has duplicated them. It then owns its
    // copies; these Kotlin descriptors can be closed immediately after configure.
    fun openFiles(files: List<SavedFile>, selected: Set<Int>): List<ParcelFileDescriptor?> {
        val handles = mutableListOf<ParcelFileDescriptor?>()
        try {
            files.forEach { f ->
                // Keep descriptors for every URI-backed file. A file may be
                // deselected today and selected again later; reopening it here
                // lets the engine keep the saved destination and bytes.
                val pfd = if (f.uri != null) {
                    val uri = f.uri ?: error("Missing saved file: ${f.name}")
                    resolver.openFileDescriptor(Uri.parse(uri), "rw") ?: error("File is unavailable: ${f.name}")
                } else null
                if (f.index in selected && pfd == null) error("Missing saved file: ${f.name}")
                handles += pfd
            }
            return handles
        } catch (t: Throwable) {
            handles.forEach { runCatching { it?.close() } }
            throw IllegalStateException("Cannot open a selected download file. Check that it still exists and folder access is allowed, then retry.", t)
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
                .put("metadata", e.metadata).put("metadataReady", e.metadataReady).put("autoSelect", e.autoSelect)
                .put("engineId", e.engineId).put("destination", e.destination)
                .put("downloadGroupUri", e.downloadGroupUri)
                .put("paused", e.paused).put("addedAt", e.addedAt)
                .put("generation", e.generation)
                .put("completionAck", e.completionAck)
                .put("focusedFile", e.focusedFile)
                .put("isDeleting", e.isDeleting)
                .put("lifecycleState", e.lifecycleState.name)
                .put("selected", JSONArray(e.selected.toList()))
                .put("files", JSONArray().apply {
                    e.files.forEach { f -> put(JSONObject().put("index", f.index).put("name", f.name)
                        .put("path", f.path).put("length", f.length).put("uri", f.uri).put("progress", f.progress)
                        .put("verifiedBytes", f.verifiedBytes)
                        .put("relativePath", f.relativePath)) }
                }))
        }
        return array.toString()
    }

    private fun parseEntry(e: JSONObject): DownloadEntry {
        val files = e.getJSONArray("files")
        val selected = e.getJSONArray("selected")
        val metadata = e.optString("metadata", "")
        val metadataReady = e.optBoolean("metadataReady", files.length() > 0 || metadata.isNotBlank())
        val stateName = e.optString("lifecycleState", "")
        val savedState = EntryLifecycleState.entries.find { it.name == stateName }
        val isDeleting = e.optBoolean("isDeleting", false)
        // Legacy Watch/stream library rows are treated as ordinary stopped downloads
        // (Watch UI was removed; keep Download + RAM write-back only).
        val legacyStream = e.optString("storageMode", "") == "STREAM" || e.optBoolean("watchOnly", false)
        val paused = e.optBoolean("paused", false) || legacyStream
        val lifecycleState = when {
            isDeleting -> EntryLifecycleState.ERROR
            legacyStream -> EntryLifecycleState.STOPPED
            savedState == EntryLifecycleState.PAUSING -> EntryLifecycleState.PAUSED
            savedState == EntryLifecycleState.STOPPING || savedState == EntryLifecycleState.PREPARING -> EntryLifecycleState.STOPPED
            savedState != null -> savedState
            paused -> EntryLifecycleState.PAUSED
            else -> EntryLifecycleState.STOPPED
        }
        val destination = if (legacyStream) {
            "Downloads/Webtor"
        } else {
            e.getString("destination")
        }
        val entry = DownloadEntry(
            e.getString("key"), e.getString("title"), e.getString("source"), metadata,
            if (legacyStream || e.isNull("engineId")) null else e.getString("engineId"),
            destination,
            (0 until files.length()).map { n -> files.getJSONObject(n).let { f ->
                val length = f.getLong("length")
                val progress = f.optDouble("progress", 0.0).let { if (it.isFinite()) it.coerceIn(0.0, 1.0) else 0.0 }
                // Older saves only stored verified-based progress; migrate into verifiedBytes.
                val verifiedBytes = when {
                    f.has("verifiedBytes") && !f.isNull("verifiedBytes") -> f.optLong("verifiedBytes", 0L)
                    length <= 0L -> 0L
                    else -> (length * progress).toLong().coerceIn(0L, length)
                }
                SavedFile(f.getInt("index"), f.getString("name"), f.getString("path"), length,
                    if (legacyStream || f.isNull("uri")) null else f.getString("uri"),
                    progress,
                    if (f.isNull("relativePath")) null else f.getString("relativePath"),
                    verifiedBytes = verifiedBytes)
            } },
            (0 until selected.length()).map { selected.getInt(it) }.toSet(),
            paused || isDeleting || savedState == EntryLifecycleState.PAUSING || savedState == EntryLifecycleState.STOPPING,
            status = null,
            error = if (isDeleting) "Deletion was interrupted. Tap Remove to retry." else null,
            addedAt = e.optLong("addedAt", 0L),
            generation = e.optLong("generation", 0L),
            isDeleting = false,
            lifecycleState = lifecycleState,
            metadataReady = metadataReady,
            autoSelect = e.optBoolean("autoSelect", true),
            completionAck = e.optLong("completionAck", if (metadataReady && selected.length() > 0) e.optLong("generation", 0L) else 0L),
            focusedFile = if (e.has("focusedFile") && !e.isNull("focusedFile")) e.optInt("focusedFile") else null,
            downloadGroupUri = e.optString("downloadGroupUri", "").ifBlank { null },
        )
        require(entry.key.isNotBlank()) { "Missing torrent identity" }
        require((!entry.metadataReady || entry.files.isNotEmpty()) && entry.files.map { it.index }.toSet().size == entry.files.size) { "Invalid torrent files" }
        require(entry.files.withIndex().all { (index, file) -> file.index == index && file.length >= 0 }) { "Invalid file index or length" }
        require(entry.selected.all { it in entry.files.indices }) { "Invalid file selection" }
        require(entry.metadataReady || entry.files.isEmpty()) { "Metadata-pending downloads cannot contain files" }
        return entry
    }

    private fun importTorrents(): List<File> = context.cacheDir.listFiles().orEmpty().filter {
        it.isFile && it.name.startsWith("import-") && it.name.endsWith(".torrent")
    }

    private fun safe(name: String): String = name.replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_")
        .trim().take(120).let { if (it.isBlank() || it == "." || it == "..") "download" else it }
    private fun mime(name: String) = mimeFor(name)

    @android.annotation.TargetApi(Build.VERSION_CODES.Q)
    private fun mediaStoreDownloadsUri(): Uri = MediaStore.Downloads.EXTERNAL_CONTENT_URI
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
