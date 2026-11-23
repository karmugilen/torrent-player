package webtor.app

/**
 * Reserve memory for Android, peer connections and decoding before sizing caches.
 * The numbers come from shared/policy.json (ramCache.auto), where the rule is written out.
 */
fun autoRamCacheMiB(
    totalBytes: Long,
    availableBytes: Long,
    lowMemoryThresholdBytes: Long,
    lowRamDevice: Boolean,
    activeTransfers: Int,
): Int {
    val auto = Policy.ramCache.auto
    if (lowRamDevice || totalBytes <= 0L) return auto.minMiB
    val reserve = maxOf(lowMemoryThresholdBytes.coerceAtLeast(0L), totalBytes / auto.reserveTotalDivisor)
    val available = (availableBytes.coerceAtLeast(0L) - reserve).coerceAtLeast(0L)
    val totalBudget = minOf(totalBytes / auto.budgetTotalDivisor, available / auto.budgetAvailableDivisor)
    val perTransferMiB = totalBudget / activeTransfers.coerceAtLeast(1) / (1024L * 1024L)
    // At most auto.maxMiB (128): measured on a 7.6 GB phone, 256 MiB pushed the whole phone into
    // swap during playback and made picture in picture lag. 128 MiB still streams smoothly.
    return auto.autoStepsMiB.firstOrNull { it <= perTransferMiB } ?: auto.minMiB
}
