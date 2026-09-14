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
import androidx.core.content.ContextCompat

/**
 * Owns the active-transfer notification independently from the Activity task.
 * The sticky foreground service keeps downloads alive after the Activity task
 * is removed. Paused downloads do not keep a service or notification alive.
 */
class PlayService : Service() {
    private var title: String = "Torrent Player"
    private var text: String = "Restoring downloads…"
    private var progress: Int = 0
    private var paused: Boolean = false
    private var isMultiple: Boolean = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        applyExtras(intent)
        return when (intent?.action) {
            ACTION_STOP -> {
                text = if (isMultiple) "Pausing downloads…" else "Pausing download…"
                showForeground(notification(commandInProgress = true))
                val session = (application as? WebtorApp)?.session
                if (session == null) stop(this) else session.pauseFromNotification()
                START_NOT_STICKY
            }
            else -> {
                val current = notification(commandInProgress = false)
                if (showForeground(current)) START_STICKY else START_NOT_STICKY
            }
        }
    }

    override fun onDestroy() {
        if (Build.VERSION.SDK_INT >= 24) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        getSystemService(NotificationManager::class.java).cancel(NOTIF_ID)
        super.onDestroy()
    }

    private fun applyExtras(intent: Intent?) {
        intent?.getStringExtra(EXTRA_TITLE)?.let { title = it }
        intent?.getStringExtra(EXTRA_TEXT)?.let { text = it }
        if (intent?.hasExtra(EXTRA_PROGRESS) == true) {
            progress = intent.getIntExtra(EXTRA_PROGRESS, 0).coerceIn(0, 100)
        }
        if (intent?.hasExtra(EXTRA_PAUSED) == true) {
            paused = intent.getBooleanExtra(EXTRA_PAUSED, false)
        }
        if (intent?.hasExtra(EXTRA_MULTIPLE) == true) {
            isMultiple = intent.getBooleanExtra(EXTRA_MULTIPLE, false)
        }
    }

    private fun showForeground(value: Notification): Boolean = runCatching {
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIF_ID, value, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIF_ID, value)
        }
        true
    }.getOrElse {
        stopSelf()
        false
    }

    private fun notification(commandInProgress: Boolean): Notification {
        createChannel()
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val open = PendingIntent.getActivity(
            this,
            REQUEST_OPEN,
            Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            flags,
        )
        val stop = serviceAction(ACTION_STOP, REQUEST_STOP, flags)
        val stopLabel = if (isMultiple) "Stop all" else "Stop"

        return NotificationCompat.Builder(this, CHANNEL)
            .setContentTitle(title)
            .setContentText(text)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentIntent(open)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setProgress(100, progress, false)
            .apply {
                if (!commandInProgress) {
                    addAction(android.R.drawable.ic_menu_close_clear_cancel, stopLabel, stop)
                }
            }
            .build()
    }

    private fun serviceAction(action: String, requestCode: Int, flags: Int): PendingIntent {
        val intent = Intent(this, PlayService::class.java)
            .setAction(action)
            .putExtra(EXTRA_TITLE, title)
            .putExtra(EXTRA_TEXT, text)
            .putExtra(EXTRA_PROGRESS, progress)
            .putExtra(EXTRA_PAUSED, paused)
            .putExtra(EXTRA_MULTIPLE, isMultiple)
        return PendingIntent.getForegroundService(this, requestCode, intent, flags)
    }

    private fun createChannel() {
        if (Build.VERSION.SDK_INT < 26) return
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Torrent download progress and controls"
                setShowBadge(false)
            },
        )
    }

    companion object {
        const val ACTION_STOP = "webtor.app.STOP"
        const val EXTRA_TITLE = "title"
        const val EXTRA_TEXT = "text"
        const val EXTRA_PROGRESS = "progress"
        const val EXTRA_PAUSED = "paused"
        const val EXTRA_MULTIPLE = "multiple"
        private const val CHANNEL = "webtor-downloads"
        private const val NOTIF_ID = 42
        private const val REQUEST_OPEN = 0
        private const val REQUEST_STOP = 3

        fun start(
            ctx: Context,
            title: String,
            progress: Int = 0,
            paused: Boolean = false,
            text: String = "Downloading",
            multiple: Boolean = false,
        ) {
            val intent = Intent(ctx, PlayService::class.java)
                .putExtra(EXTRA_TITLE, title)
                .putExtra(EXTRA_PROGRESS, progress)
                .putExtra(EXTRA_PAUSED, paused)
                .putExtra(EXTRA_TEXT, text)
                .putExtra(EXTRA_MULTIPLE, multiple)
            runCatching { ContextCompat.startForegroundService(ctx, intent) }
        }

        fun stop(ctx: Context) {
            ctx.getSystemService(NotificationManager::class.java).cancel(NOTIF_ID)
            ctx.stopService(Intent(ctx, PlayService::class.java))
        }
    }
}
