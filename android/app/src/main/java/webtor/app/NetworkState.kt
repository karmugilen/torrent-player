package webtor.app

/** Platform-independent ordering and deduplication of default-network events. */
data class NetworkSnapshot(
    val connected: Boolean = false,
    val wifi: Boolean = false,
    val metered: Boolean = true,
    val generation: Long = 0,
)

class DefaultNetworkState {
    private var network: Long? = null
    private var usable = false
    private var blocked = false
    private var wifi = false
    private var metered = true
    private var links: String? = null
    private var publishedRoute: Pair<Long, String?>? = null
    var snapshot = NetworkSnapshot()
        private set

    fun available(id: Long): NetworkSnapshot {
        if (network != id) {
            network = id
            usable = false
            blocked = false
            wifi = false
            metered = true
            links = null
        }
        return publish()
    }

    fun capabilities(id: Long, validated: Boolean, wifi: Boolean, metered: Boolean): NetworkSnapshot {
        if (network != id) return snapshot
        usable = validated
        this.wifi = wifi
        this.metered = metered
        return publish()
    }

    fun links(id: Long, signature: String): NetworkSnapshot {
        if (network != id) return snapshot
        links = signature
        return publish()
    }

    fun blocked(id: Long, value: Boolean): NetworkSnapshot {
        if (network != id) return snapshot
        blocked = value
        return publish()
    }

    fun lost(id: Long): NetworkSnapshot {
        if (network != id) return snapshot
        network = null
        usable = false
        links = null
        return publish()
    }

    private fun publish(): NetworkSnapshot {
        val connected = network != null && usable && !blocked
        val route = if (connected) network!! to links else null
        val generation = snapshot.generation + if (publishedRoute != route) 1 else 0
        publishedRoute = route
        snapshot = NetworkSnapshot(connected, wifi, metered, generation)
        return snapshot
    }
}
