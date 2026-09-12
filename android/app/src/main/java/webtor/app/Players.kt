package webtor.app

import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import java.util.Locale

data class PlayerApp(
    val packageName: String,
    val activity: String,
    val label: String,
)

fun queryVideoPlayers(pm: PackageManager, selfPackage: String): List<PlayerApp> {
    val probe = Intent(Intent.ACTION_VIEW).setDataAndType(
        Uri.parse("http://127.0.0.1/stream.mp4"),
        "video/*",
    )
    val flags = if (Build.VERSION.SDK_INT >= 23) PackageManager.MATCH_ALL else 0
    return pm.queryIntentActivities(probe, flags)
        .mapNotNull { info ->
            val activity = info.activityInfo ?: return@mapNotNull null
            if (!activity.exported) return@mapNotNull null
            if (activity.packageName == selfPackage) return@mapNotNull null
            PlayerApp(
                packageName = activity.packageName,
                activity = activity.name,
                label = info.loadLabel(pm).toString().ifBlank { activity.packageName },
            )
        }
        .distinctBy { it.packageName }
        .sortedBy { it.label.lowercase(Locale.US) }
}

fun findPlayer(players: List<PlayerApp>, packageName: String): PlayerApp? {
    if (packageName.isBlank()) return null
    return players.find { it.packageName == packageName }
}
