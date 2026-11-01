package webtor.app

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Receives magnets and torrents from other apps, and hands them to [MainActivity]
 * in this app's own task. Without it the library opens inside the other app's
 * task. MainActivity stays a standard activity: singleTask would close the video
 * player above it whenever the launcher icon is tapped.
 */
class IncomingActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val grants = intent.flags and
            (Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        startActivity(
            Intent(intent)
                .setClass(this, MainActivity::class.java)
                .setFlags(
                    grants or Intent.FLAG_ACTIVITY_NEW_TASK or
                        Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP,
                ),
        )
        finish()
    }
}
