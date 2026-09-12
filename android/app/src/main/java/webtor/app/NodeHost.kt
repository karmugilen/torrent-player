package webtor.app

import android.content.Context
import android.util.Log
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.atomic.AtomicBoolean

class NodeHost(private val context: Context) {
    private val started = AtomicBoolean(false)

    fun projectDir(): File = File(context.filesDir, "nodejs-project")
    fun downloadDir(): File = File(context.filesDir, "webtorrent")

    fun start(ctlPort: Int = DEFAULT_CTL_PORT) {
        if (!started.compareAndSet(false, true)) return
        val maxPeers = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
            .getInt("maxPeers", 55)
            .coerceIn(8, 80)
        Thread({
            try {
                syncAssets()
                val project = projectDir().absolutePath
                val tmp = context.cacheDir.absolutePath
                val code = startNodeWithArguments(
                    arrayOf("node", "$project/main.js"),
                    arrayOf(
                        "WEBTOR_CTL_PORT=$ctlPort",
                        "WEBTOR_STREAM_PORT=0",
                        "WEBTOR_PATH=${downloadDir().absolutePath}",
                        "WEBTOR_MAX_PEERS=$maxPeers",
                        "WEBTOR_NATIVE_LIBDIR=${context.applicationInfo.nativeLibraryDir}",
                        "HOME=${context.filesDir.absolutePath}",
                        "TMPDIR=$tmp",
                        "NODE_PATH=$project/node_modules",
                    ),
                    project,
                )
                Log.i(TAG, "node exited $code")
            } catch (t: Throwable) {
                Log.e(TAG, "node failed", t)
            }
        }, "webtor-node").start()
    }

    private fun syncAssets() {
        downloadDir().mkdirs()
        val dest = projectDir()
        val bundledRev = runCatching {
            context.assets.open("nodejs-project/bundle.rev").bufferedReader().use { it.readText() }
        }.getOrNull()
        val installed = File(dest, "bundle.rev")
        if (bundledRev != null && File(dest, "main.js").isFile && installed.isFile && installed.readText() == bundledRev) {
            return
        }
        dest.deleteRecursively()
        dest.mkdirs()
        copyAssetDir("nodejs-project", dest)
    }

    private fun copyAssetDir(assetPath: String, dest: File) {
        val am = context.assets
        val kids = am.list(assetPath) ?: return
        if (kids.isEmpty()) {
            dest.parentFile?.mkdirs()
            am.open(assetPath).use { input ->
                FileOutputStream(File(dest.parentFile, dest.name)).use { input.copyTo(it) }
            }
            return
        }
        dest.mkdirs()
        for (name in kids) {
            copyAssetDir("$assetPath/$name", File(dest, name))
        }
    }

    private external fun startNodeWithArguments(
        arguments: Array<String>,
        environPairs: Array<String>,
        cwdPath: String,
    ): Int

    companion object {
        const val TAG = "webtor-node"
        const val DEFAULT_CTL_PORT = 18080

        init {
            System.loadLibrary("node")
            System.loadLibrary("native-lib")
        }
    }
}
