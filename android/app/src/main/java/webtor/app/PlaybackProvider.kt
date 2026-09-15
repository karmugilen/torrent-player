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

    private fun target(uri: Uri): Pair<DownloadEntry, SavedFile> {
        val parts = uri.pathSegments
        if (parts.size != 3) throw FileNotFoundException("Invalid playback URI")
        val app = context!!.applicationContext as WebtorApp
        val entry = app.session.ui.value.library.find { it.key == parts[0] }
            ?: DownloadStorage(app).load().find { it.key == parts[0] }
            ?: throw FileNotFoundException("Download was removed")
        val index = parts[1].toIntOrNull()
        val file = entry.files.find { it.index == index && it.index in entry.selected }
            ?: throw FileNotFoundException("File is not selected")
        if (entry.isDeleting || file.uri == null) throw FileNotFoundException("File is unavailable")
        return entry to file
    }

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
            if (file.progress >= 1.0 || file.length == 0L) {
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
                            app.engine.closePlayback(handle)
                            thread.quitSafely()
                        }
                    },
                    Handler(thread.looper),
                )
            } catch (t: Throwable) {
                app.engine.closePlayback(handle)
                thread.quitSafely()
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
        fun uriFor(entry: DownloadEntry, file: SavedFile): Uri = Uri.Builder()
            .scheme("content").authority("webtor.app.playback")
            .appendPath(entry.key).appendPath(file.index.toString()).appendPath(file.name).build()
    }
}
