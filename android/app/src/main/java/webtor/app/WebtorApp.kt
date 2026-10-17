package webtor.app

import android.app.Activity
import android.app.Application
import java.lang.ref.WeakReference
import webtor.core.EngineClient
import android.net.ConnectivityManager
import android.util.Log

class WebtorApp : Application() {
    val playbackLeases = PlaybackLeases { if (::session.isInitialized) session.refreshTransferNotification() }
    private var networkMonitor: NetworkMonitor? = null
    lateinit var engine: EngineHost
        private set
    lateinit var engineClient: EngineClient
        private set
    lateinit var session: LibrarySession
        private set
    lateinit var thumbnails: ThumbnailRepository
        private set

    @Volatile private var activityRef: WeakReference<Activity>? = null

    fun setCurrentActivity(activity: Activity) {
        activityRef = WeakReference(activity)
    }

    fun clearCurrentActivity(activity: Activity) {
        if (activityRef?.get() === activity) activityRef = null
    }

    fun currentActivity(): Activity? = activityRef?.get()

    override fun onCreate() {
        super.onCreate()
        engine = EngineHost(this)
        engineClient = EngineClient(engine)
        session = LibrarySession(this, engineClient, engine)
        thumbnails = ThumbnailRepository(this, engine)
        engine.start()
        session.start()
        val cm = getSystemService(ConnectivityManager::class.java)
        networkMonitor = NetworkMonitor(cm, session::updateNetworkState)
        runCatching { networkMonitor!!.start() }
            .onFailure { Log.e(EngineHost.TAG, "Could not monitor network changes", it) }
    }
}
