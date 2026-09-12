package webtor.app

import android.app.Application

class WebtorApp : Application() {
    lateinit var node: NodeHost
        private set
    lateinit var session: LibrarySession
        private set

    override fun onCreate() {
        super.onCreate()
        node = NodeHost(this)
        session = LibrarySession(this)
        node.start()
        session.start()
    }
}
