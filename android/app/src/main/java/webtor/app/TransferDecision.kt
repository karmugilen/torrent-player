package webtor.app

/** SharedPreferences keys from the removed download-slot queue; cleared on upgrade. */
val LEGACY_DOWNLOAD_QUEUE_PREF_KEYS = listOf("downloadQueueMode", "downloadQueueOrder")

/**
 * Whether restore/configure should apply the saved selected-file intent as
 * native payload selection. User-paused torrents are never auto-started.
 */
fun shouldRestorePayloadSelection(
    paused: Boolean,
    isDeleting: Boolean,
    complete: Boolean,
    metadataReady: Boolean,
    hasSelection: Boolean,
): Boolean = !paused && !isDeleting && !complete && metadataReady && hasSelection

/**
 * The user-owned state of a transfer.  This is deliberately separate from an
 * engine status: a temporary engine, tracker, or network failure must never
 * turn a requested download into a user pause.
 */
enum class TransferIntent {
    RUN,
    PAUSED,
    DELETED,
}

/** Whether the app can safely write torrent payload bytes. */
enum class StorageReadiness {
    READY,
    ACCESS_REQUIRED,
    FATAL_ERROR,
    TRANSIENT_ERROR,
}

/**
 * All facts needed to decide what the engine may do for one entry.  Keeping
 * this data-only makes the policy easy to unit test and prevents UI wording
 * from changing transfer behaviour.
 */
data class TransferDecisionInput(
    val intent: TransferIntent,
    val engineReady: Boolean,
    val metadataReady: Boolean,
    val hasSelection: Boolean,
    val storage: StorageReadiness,
    val networkAllowed: Boolean,
    val verificationActive: Boolean,
)

enum class TransferStage {
    DELETED,
    PAUSED,
    STARTING_ENGINE,
    WAITING_FOR_NETWORK,
    FINDING_METADATA,
    VERIFYING_SAVED_DATA,
    WAITING_FOR_SELECTION,
    STORAGE_ACCESS_REQUIRED,
    STORAGE_ERROR,
    RETRYING,
    DOWNLOADING,
}

/**
 * Discovery covers DHT/tracker/peer metadata discovery.  Payload means piece
 * requests may write file data.  The caller maps these permissions to native
 * engine calls; this policy deliberately owns no timer or coroutine.
 */
data class TransferDecision(
    val discoveryAllowed: Boolean,
    val payloadAllowed: Boolean,
    val stage: TransferStage,
    val visibleReason: String,
)

/** Pure, shared permission policy for add, restore, resume, and retry paths. */
fun decideTransfer(input: TransferDecisionInput): TransferDecision = when {
    input.intent == TransferIntent.DELETED -> decision(
        TransferStage.DELETED, "Removing download",
    )
    input.intent == TransferIntent.PAUSED -> decision(
        TransferStage.PAUSED, "Paused",
    )
    !input.engineReady -> decision(
        TransferStage.STARTING_ENGINE, "Starting download engine",
    )
    !input.networkAllowed -> decision(
        TransferStage.WAITING_FOR_NETWORK, "Waiting for allowed network",
    )
    // A magnet has no file selection before metadata exists.  Keep discovery
    // alive so it can become a real download when a peer later appears.
    !input.metadataReady -> decision(
        TransferStage.FINDING_METADATA, "Finding peers and file list", discovery = true,
    )
    input.verificationActive -> decision(
        TransferStage.VERIFYING_SAVED_DATA, "Checking saved data", discovery = true,
    )
    !input.hasSelection -> decision(
        TransferStage.WAITING_FOR_SELECTION, "No files selected",
    )
    input.storage == StorageReadiness.ACCESS_REQUIRED -> decision(
        TransferStage.STORAGE_ACCESS_REQUIRED, "Folder access is required",
    )
    input.storage == StorageReadiness.FATAL_ERROR -> decision(
        TransferStage.STORAGE_ERROR, "Storage needs attention",
    )
    input.storage == StorageReadiness.TRANSIENT_ERROR -> decision(
        TransferStage.RETRYING, "Will retry shortly",
    )
    else -> TransferDecision(
        discoveryAllowed = true,
        payloadAllowed = true,
        stage = TransferStage.DOWNLOADING,
        visibleReason = "Downloading",
    )
}

private fun decision(
    stage: TransferStage,
    visibleReason: String,
    discovery: Boolean = false,
) = TransferDecision(
    discoveryAllowed = discovery,
    payloadAllowed = false,
    stage = stage,
    visibleReason = visibleReason,
)

/** A unique, monotonically increasing identity supplied by the entry owner. */
data class RetryEpisode(
    val id: Long,
    val generation: Long,
)

/**
 * Persist this only as long as a retry is pending.  [attempt] is one-based and
 * includes the retry currently scheduled or running.
 */
data class RetryState(
    val episode: RetryEpisode,
    val attempt: Int,
    val scheduled: Boolean,
    val networkRecoveryUsed: Boolean = false,
) {
    init {
        require(attempt in 1..RetryPolicy.MAX_ATTEMPTS)
    }
}

sealed interface RetryDirective {
    /** The owner should arrange one callback after [afterMillis]. */
    data class Schedule(val state: RetryState, val afterMillis: Long) : RetryDirective
    /** A previously scheduled callback remains the only valid callback. */
    data object AlreadyScheduled : RetryDirective
    /** The event belongs to a deleted/replaced retry episode. */
    data object IgnoreStaleEvent : RetryDirective
    /** All automatic attempts for this episode have been used. */
    data object Exhausted : RetryDirective
    /** Cancel any owner timer and forget its state. */
    data object Cancel : RetryDirective
    /** No timer change is required. */
    data object NoChange : RetryDirective
}

/**
 * Bounded retry scheduling without a timer.  Callers must retain the returned
 * [RetryState], cancel their timer on [RetryDirective.Cancel], and attach its
 * episode and generation to asynchronous callbacks before calling back here.
 */
object RetryPolicy {
    const val MAX_ATTEMPTS = 3
    private val retryDelaysMillis = longArrayOf(5_000L, 15_000L, 45_000L)

    /**
     * Records a transient failure.  A second failure while a retry is already
     * scheduled is deduplicated.  After the scheduled retry has started,
     * [markAttemptStarted] flips [RetryState.scheduled] and the next failure
     * may schedule the following bounded retry.
     */
    fun onTransientFailure(
        current: RetryState?,
        event: RetryEpisode,
        intent: TransferIntent,
    ): RetryDirective {
        if (intent != TransferIntent.RUN) return if (current != null) RetryDirective.Cancel else RetryDirective.NoChange
        if (current != null && current.episode != event) return RetryDirective.IgnoreStaleEvent

        return when {
            current == null -> schedule(event, attempt = 1)
            current.scheduled -> RetryDirective.AlreadyScheduled
            current.attempt >= MAX_ATTEMPTS -> RetryDirective.Exhausted
            else -> schedule(
                episode = event,
                attempt = current.attempt + 1,
                networkRecoveryUsed = current.networkRecoveryUsed,
            )
        }
    }

    /** The owner calls this exactly when its scheduled callback begins work. */
    fun markAttemptStarted(current: RetryState, event: RetryEpisode): RetryState? =
        if (current.episode == event && current.scheduled) current.copy(scheduled = false) else null

    /**
     * Network recovery may pull one pending retry forward immediately.  It
     * cannot create another retry and it applies at most once per episode.
     */
    fun onNetworkRecovered(
        current: RetryState?,
        event: RetryEpisode,
        intent: TransferIntent,
    ): RetryDirective {
        if (intent != TransferIntent.RUN) return if (current != null) RetryDirective.Cancel else RetryDirective.NoChange
        if (current == null || current.episode != event) return RetryDirective.IgnoreStaleEvent
        if (!current.scheduled || current.networkRecoveryUsed) return RetryDirective.NoChange
        return RetryDirective.Schedule(
            state = current.copy(networkRecoveryUsed = true),
            afterMillis = 0L,
        )
    }

    /** Pause and delete always cancel a pending automatic retry. */
    fun onIntentChanged(current: RetryState?, intent: TransferIntent): RetryDirective = when {
        current == null -> RetryDirective.NoChange
        intent == TransferIntent.RUN -> RetryDirective.NoChange
        else -> RetryDirective.Cancel
    }

    private fun schedule(
        episode: RetryEpisode,
        attempt: Int,
        networkRecoveryUsed: Boolean = false,
    ) = RetryDirective.Schedule(
        state = RetryState(
            episode = episode,
            attempt = attempt,
            scheduled = true,
            networkRecoveryUsed = networkRecoveryUsed,
        ),
        afterMillis = retryDelaysMillis[attempt - 1],
    )
}
