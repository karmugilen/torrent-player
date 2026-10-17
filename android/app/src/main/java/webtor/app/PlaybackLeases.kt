package webtor.app

import java.util.concurrent.atomic.AtomicBoolean

/** Each external proxy descriptor retains its own process lifetime lease. */
class PlaybackLeases(private val onChanged: () -> Unit = {}) {
    private val counts = mutableMapOf<String, Int>()

    fun activeKeys(): Set<String> = synchronized(counts) { counts.keys.toSet() }

    fun acquire(key: String): AutoCloseable {
        synchronized(counts) { counts[key] = (counts[key] ?: 0) + 1 }
        onChanged()
        val released = AtomicBoolean(false)
        return AutoCloseable {
            if (released.compareAndSet(false, true)) {
                synchronized(counts) {
                    val remaining = (counts[key] ?: 1) - 1
                    if (remaining == 0) counts.remove(key) else counts[key] = remaining
                }
                onChanged()
            }
        }
    }
}
