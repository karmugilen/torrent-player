package webtor.app

/** Reserve memory for Android, peer connections and decoding before sizing caches. */
fun autoRamCacheMiB(
    totalBytes: Long,
    availableBytes: Long,
    lowMemoryThresholdBytes: Long,
    lowRamDevice: Boolean,
    activeTransfers: Int,
): Int {
    if (lowRamDevice || totalBytes <= 0L) return 64
    val reserve = maxOf(lowMemoryThresholdBytes.coerceAtLeast(0L), totalBytes / 10L)
    val available = (availableBytes.coerceAtLeast(0L) - reserve).coerceAtLeast(0L)
    val totalBudget = minOf(totalBytes / 16L, available / 4L)
    val perTransferMiB = totalBudget / activeTransfers.coerceAtLeast(1) / (1024L * 1024L)
    // At most 128 MiB: measured on a 7.6 GB phone, 256 MiB pushed the whole phone into swap
    // during playback and made picture in picture lag. 128 MiB still streams smoothly.
    return listOf(128, 64).first { it <= perTransferMiB || it == 64 }
}
