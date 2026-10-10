package bot.mac.mobile.core.state

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Coalesces durable event snapshots without moving the durable cursor ahead of
 * the snapshot that contains it. The caller's flush callback must write the
 * snapshot before recording [seq] as the persisted cursor.
 */
internal class DurableSnapshotGate(
    private val scope: CoroutineScope,
    private val batchSize: Int = 32,
    private val delayMillis: Long = 250L,
    private val flush: suspend (seq: Long) -> Unit,
) {
    private val mutex = Mutex()
    private var pendingSeq: Long? = null
    private var pendingCount = 0
    private var timer: Job? = null

    suspend fun record(seq: Long) {
        var flushNow = false
        mutex.withLock {
            pendingSeq = maxOf(pendingSeq ?: seq, seq)
            pendingCount++
            if (pendingCount >= batchSize) {
                timer?.cancel()
                timer = null
                flushNow = true
            } else if (timer == null) {
                timer = scope.launch {
                    delay(delayMillis)
                    flushPending()
                }
            }
        }
        if (flushNow) flushPending()
    }

    suspend fun flushPending() {
        val seq = mutex.withLock {
            val value = pendingSeq
            pendingSeq = null
            pendingCount = 0
            timer = null
            value
        } ?: return
        flush(seq)
    }

    suspend fun close() {
        mutex.withLock {
            timer?.cancel()
            timer = null
        }
        flushPending()
    }
}
