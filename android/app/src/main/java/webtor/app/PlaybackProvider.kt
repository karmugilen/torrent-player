package webtor.app

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Binder
import android.os.Handler
import android.os.HandlerThread
import android.os.ParcelFileDescriptor
import android.os.ProxyFileDescriptorCallback
import android.os.storage.StorageManager
import android.provider.OpenableColumns
import android.system.ErrnoException
import android.system.OsConstants
import java.io.FileNotFoundException

/** A read grant exposes one selected file, with stable size and seek support. */
class PlaybackProvider : ContentProvider() {
    override fun onCreate() = true

    private fun target(uri: Uri): Pair<DownloadEntry, SavedFile> =
        target(context!!.applicationContext as WebtorApp, uri)

    override fun getType(uri: Uri): String = mimeFor(target(uri).second.name)

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?,
                       selectionArgs: Array<out String>?, sortOrder: String?): Cursor {
        val file = target(uri).second
        val columns = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return MatrixCursor(columns).apply {
            addRow(columns.map { column ->
                when (column) {
                    OpenableColumns.DISPLAY_NAME -> file.name
                    OpenableColumns.SIZE -> file.length
                    else -> null
                }
            }.toTypedArray())
        }
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw FileNotFoundException("Playback is read-only")
        val identity = Binder.clearCallingIdentity()
        try {
            val (entry, file) = target(uri)
            val app = context!!.applicationContext as WebtorApp
            if (!entry.directPlay && file.isVerifiedComplete) {
                val descriptor = app.contentResolver.openFileDescriptor(Uri.parse(file.uri), "r")
                    ?: throw FileNotFoundException("Downloaded file is unavailable")
                if (descriptor.statSize >= 0 && descriptor.statSize < file.length) {
                    descriptor.close()
                    throw FileNotFoundException("Downloaded file is shorter than expected; resume it first")
                }
                return descriptor
            }
            val id = entry.engineId ?: throw FileNotFoundException("Resume the download before playing")
            val handle = app.engine.openPlayback(id, file.index)
            if (handle == 0L) throw FileNotFoundException("Cannot open torrent stream")
            val lease = app.playbackLeases.acquire(entry.key)
            // A blocked read must not block another player's seek or UI commands.
            val thread = HandlerThread("torrent-playback").apply { start() }
            try {
                return app.getSystemService(StorageManager::class.java).openProxyFileDescriptor(
                    ParcelFileDescriptor.MODE_READ_ONLY,
                    object : ProxyFileDescriptorCallback() {
                        override fun onGetSize() = file.length
                        override fun onRead(offset: Long, size: Int, data: ByteArray): Int {
                            val count = app.engine.readPlayback(handle, offset, size, data)
                            if (count < 0) throw ErrnoException("torrent read", OsConstants.EIO)
                            return count
                        }
                        override fun onRelease() {
                            try { app.engine.closePlayback(handle) } finally { lease.close(); thread.quitSafely() }
                        }
                    },
                    Handler(thread.looper),
                )
            } catch (t: Throwable) {
                try { app.engine.closePlayback(handle) } finally { lease.close(); thread.quitSafely() }
                throw FileNotFoundException("Cannot open torrent stream: ${t.message}")
            }
        } finally {
            Binder.restoreCallingIdentity(identity)
        }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri = throw UnsupportedOperationException("Read-only")
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0

    companion object {
        private fun target(app: WebtorApp, uri: Uri): Pair<DownloadEntry, SavedFile> {
            val parts = uri.pathSegments
            if (parts.size != 3) throw FileNotFoundException("Invalid playback URI")
            val entry = app.session.ui.value.library.find { it.key == parts[0] }
                ?: DownloadStorage(app).load().find { it.key == parts[0] }
                ?: throw FileNotFoundException("Download was removed")
            val index = parts[1].toIntOrNull()
            val file = entry.files.find {
                it.index == index && (it.index in entry.selected || (!entry.directPlay && it.isVerifiedComplete))
            }
                ?: throw FileNotFoundException("File is not selected")
            if (entry.isDeleting || (!entry.directPlay && file.uri == null)) throw FileNotFoundException("File is unavailable")
            return entry to file
        }

        fun isOwn(uri: Uri): Boolean = uri.scheme == "content" && uri.authority == "${BuildConfig.APPLICATION_ID}.playback"

        /**
         * Read one of our own playback files from inside this app, never through [openFile].
         * Its proxy descriptor is served by this same process: a read waiting on it when the
         * app is force-stopped leaves a thread stuck in the kernel, and Android then cannot
         * start the app again until the phone restarts.
         */
        fun openInApp(app: WebtorApp, uri: Uri): java.io.InputStream {
            val (entry, file) = target(app, uri)
            if (!entry.directPlay && file.isVerifiedComplete) {
                return app.contentResolver.openInputStream(Uri.parse(file.uri))
                    ?: throw FileNotFoundException("Downloaded file is unavailable")
            }
            val id = entry.engineId ?: throw FileNotFoundException("Resume the download before playing")
            val handle = app.engine.openPlayback(id, file.index)
            if (handle == 0L) throw FileNotFoundException("Cannot open torrent stream")
            val lease = app.playbackLeases.acquire(entry.key)
            return object : java.io.InputStream() {
                private var pos = 0L
                private var closed = false
                override fun read(): Int {
                    val one = ByteArray(1)
                    return if (read(one, 0, 1) <= 0) -1 else one[0].toInt() and 0xff
                }
                override fun read(b: ByteArray, off: Int, len: Int): Int {
                    if (closed) throw java.io.IOException("closed")
                    if (len == 0) return 0
                    if (pos >= file.length) return -1
                    val want = minOf(len.toLong(), file.length - pos, 1L shl 20).toInt()
                    val buf = if (off == 0) b else ByteArray(want)
                    val n = app.engine.readPlayback(handle, pos, want, buf)
                    if (n < 0) throw java.io.IOException("torrent read failed")
                    if (n == 0) return -1
                    if (buf !== b) System.arraycopy(buf, 0, b, off, n)
                    pos += n
                    return n
                }
                override fun close() {
                    if (closed) return
                    closed = true
                    try { app.engine.closePlayback(handle) } finally { lease.close() }
                }
            }
        }

        fun uriFor(entry: DownloadEntry, file: SavedFile): Uri = Uri.Builder()
            .scheme("content").authority("${BuildConfig.APPLICATION_ID}.playback")
            .appendPath(entry.key).appendPath(file.index.toString()).appendPath(file.name).build()
    }
}
