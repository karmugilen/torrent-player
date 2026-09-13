package webtor.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import java.util.concurrent.atomic.AtomicBoolean

class PlayService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP, ACTION_DISMISS -> {
                val app = application as? WebtorApp
                if (app != null) {
                    app.session.stopAllAndExit {
                        stopForeground(STOP_FOREGROUND_REMOVE)
                        stopSelf()
                        running.set(false)
                    }
                } else {
                    stopForeground(STOP_FOREGROUND_REMOVE)
                    stopSelf()
                    running.set(false)
                }
                return START_NOT_STICKY
            }
            ACTION_PAUSE -> (application as? WebtorApp)?.session?.pauseFromNotification()
            ACTION_RESUME -> (application as? WebtorApp)?.session?.resumeFromNotification()
        }
        intent?.getStringExtra(EXTRA_TITLE)?.let { title = it }
        intent?.getStringExtra(EXTRA_TEXT)?.let { text = it }
        if (intent?.hasExtra(EXTRA_PROGRESS) == true) progress = intent.getIntExtra(EXTRA_PROGRESS, 0)
        if (intent?.hasExtra(EXTRA_PAUSED) == true) paused = intent.getBooleanExtra(EXTRA_PAUSED, false)
        if (intent?.hasExtra(EXTRA_MULTIPLE) == true) isMultiple = intent.getBooleanExtra(EXTRA_MULTIPLE, false)
        running.set(true)
        val notification = notification()
        val foreground = runCatching {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTIF_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            } else {
                startForeground(NOTIF_ID, notification)
            }
            true
        }.getOrDefault(false)
        if (paused || !foreground) {
            if (foreground) {
                stopForeground(STOP_FOREGROUND_DETACH)
            }
            getSystemService(NotificationManager::class.java).notify(NOTIF_ID, notification)
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running.set(false)
        super.onDestroy()
    }

    private fun notification(): Notification {
        val nm = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW),
            )
        }
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), flags,
        )
        val pause = PendingIntent.getService(
            this, 1, Intent(this, PlayService::class.java).setAction(ACTION_PAUSE), flags,
        )
        val resume = PendingIntent.getService(
            this, 2, Intent(this, PlayService::class.java).setAction(ACTION_RESUME), flags,
        )
        val stop = PendingIntent.getService(
            this, 3, Intent(this, PlayService::class.java).setAction(ACTION_STOP), flags,
        )
        val dismiss = PendingIntent.getService(
            this, 4, Intent(this, PlayService::class.java).setAction(ACTION_DISMISS), flags,
        )
        val pauseLabel = if (isMultiple) "Pause all" else "Pause"
        val resumeLabel = if (isMultiple) "Resume all" else "Resume"
        val stopLabel = if (isMultiple) "Stop all" else "Stop"

        val builder = NotificationCompat.Builder(this, CHANNEL)
            .setContentTitle(title)
            .setContentText(text)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentIntent(open)
            .setDeleteIntent(dismiss)
            .setOngoing(!paused)
            .setOnlyAlertOnce(true)
            .setProgress(100, progress.coerceIn(0, 100), false)
            .addAction(
                if (paused) android.R.drawable.ic_media_play else android.R.drawable.ic_media_pause,
                if (paused) resumeLabel else pauseLabel,
                if (paused) resume else pause,
            )
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, stopLabel, stop)
        return builder.build()
    }

    private var title: String = "Torrent Player"
    private var text: String = "Downloading"
    private var progress: Int = 0
    private var paused: Boolean = false
    private var isMultiple: Boolean = false

    companion object {
        const val ACTION_STOP = "webtor.app.STOP"
        const val ACTION_DISMISS = "webtor.app.DISMISS"
        const val ACTION_PAUSE = "webtor.app.PAUSE"
        const val ACTION_RESUME = "webtor.app.RESUME"
        const val EXTRA_TITLE = "title"
        const val EXTRA_TEXT = "text"
        const val EXTRA_PROGRESS = "progress"
        const val EXTRA_PAUSED = "paused"
        const val EXTRA_MULTIPLE = "multiple"
        private const val CHANNEL = "webtor-downloads"
        private const val NOTIF_ID = 42
        private val running = AtomicBoolean(false)

        fun start(
            ctx: Context,
            title: String,
            progress: Int = 0,
            paused: Boolean = false,
            text: String = "Downloading",
            multiple: Boolean = false,
        ) {
            val i = Intent(ctx, PlayService::class.java)
                .putExtra(EXTRA_TITLE, title)
                .putExtra(EXTRA_PROGRESS, progress)
                .putExtra(EXTRA_PAUSED, paused)
                .putExtra(EXTRA_TEXT, text)
                .putExtra(EXTRA_MULTIPLE, multiple)
            try {
                if (running.get()) {
                    ctx.startService(i)
                } else {
                    ctx.startForegroundService(i)
                }
            } catch (_: RuntimeException) {
                running.set(false)
            }
        }

        fun stop(ctx: Context) {
            if (!running.getAndSet(false)) return
            ctx.stopService(Intent(ctx, PlayService::class.java))
        }
    }
}
