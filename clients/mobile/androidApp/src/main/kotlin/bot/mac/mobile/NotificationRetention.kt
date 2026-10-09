package bot.mac.mobile

/** The Android notification identity used by NotificationManager. */
data class NotificationIdentity(
    val tag: String?,
    val id: Int,
)

/** Metadata needed to choose an existing notification for eviction. */
data class RetainedNotification(
    val identity: NotificationIdentity,
    val postedAt: Long,
    val priority: Int = 0,
    val `protected`: Boolean = false,
)

/** Pure planning for keeping a safety margin below Android's package quota. */
object NotificationRetention {
    const val MAX_ACTIVE = 40

    /**
     * Returns existing notification identities that should be cancelled before
     * posting [incoming]. The caller supplies all active notifications,
     * including the foreground service and summary notification.
     */
    fun evictions(
        active: List<RetainedNotification>,
        incoming: NotificationIdentity,
    ): List<NotificationIdentity> {
        val incomingAlreadyActive = active.any { it.identity == incoming }
        val incomingSlots = if (incomingAlreadyActive) 0 else 1
        val required = (active.size + incomingSlots - MAX_ACTIVE)
            .coerceAtLeast(0)
        if (required == 0) return emptyList()

        return active.withIndex()
            .asSequence()
            .filter { it.value.identity != incoming && !it.value.`protected` }
            .sortedWith(
                compareBy<IndexedValue<RetainedNotification>> { it.value.priority }
                    .thenBy { it.value.postedAt }
                    // Keep equal timestamps fair and deterministic by retaining
                    // the caller's original order as the final tie-breaker.
                    .thenBy { it.index },
            )
            .take(required)
            .map { it.value.identity }
            .toList()
    }
}
