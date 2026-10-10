package bot.mac.mobile.core.state

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.ExperimentalCoroutinesApi

@OptIn(ExperimentalCoroutinesApi::class)
class DurableSnapshotGateTest {
    @Test
    fun coalescesBurstAndFlushesLatestSequence() = runTest {
        val flushed = mutableListOf<Long>()
        val gate = DurableSnapshotGate(this, batchSize = 4, delayMillis = 250) { flushed += it }

        gate.record(7)
        gate.record(8)
        gate.record(9)
        assertEquals(emptyList(), flushed)
        advanceTimeBy(250)
        advanceUntilIdle()
        assertEquals(listOf(9L), flushed)
    }

    @Test
    fun batchFlushDoesNotLoseLaterSequenceAndCloseFlushesPending() = runTest {
        val flushed = mutableListOf<Long>()
        val gate = DurableSnapshotGate(this, batchSize = 2, delayMillis = 250) { flushed += it }

        gate.record(10)
        gate.record(12)
        advanceUntilIdle()
        gate.record(13)
        gate.close()

        assertEquals(listOf(12L, 13L), flushed)
    }
}
