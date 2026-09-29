package webtor.app

import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper

/** Process-owned callback; Android delivers capabilities after onAvailable. */
class NetworkMonitor(
    private val manager: ConnectivityManager,
    private val onChange: (NetworkSnapshot) -> Unit,
) : ConnectivityManager.NetworkCallback() {
    private val state = DefaultNetworkState()
    private var published: NetworkSnapshot? = null

    fun start() {
        publish(state.snapshot)
        manager.registerDefaultNetworkCallback(this, Handler(Looper.getMainLooper()))
    }

    override fun onAvailable(network: Network) = publish(state.available(network.networkHandle))

    override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = publish(
        state.capabilities(
            network.networkHandle,
            validated = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
                caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED),
            wifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
            metered = !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
        ),
    )

    override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) = publish(
        state.links(network.networkHandle, listOf(
            properties.interfaceName.orEmpty(),
            properties.linkAddresses.map { it.toString() }.sorted().joinToString(),
            properties.routes.map { it.toString() }.sorted().joinToString(),
            properties.dnsServers.map { it.hostAddress.orEmpty() }.sorted().joinToString(),
        ).joinToString("|")),
    )

    override fun onBlockedStatusChanged(network: Network, blocked: Boolean) =
        publish(state.blocked(network.networkHandle, blocked))

    override fun onLost(network: Network) = publish(state.lost(network.networkHandle))

    private fun publish(snapshot: NetworkSnapshot) {
        if (published == snapshot) return
        published = snapshot
        onChange(snapshot)
    }
}
