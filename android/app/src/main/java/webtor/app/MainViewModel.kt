package webtor.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel

class MainViewModel(app: Application) : AndroidViewModel(app) {
    val session: LibrarySession = getApplication<WebtorApp>().session
    val ui = session.ui
    val events = session.events
}
