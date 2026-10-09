package bot.mac.mobile.feature.computer

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest

class InputEventQueueTest {
    @Test
    fun transientMotionMayDropButImportantEventsWaitAndArrive() = runTest {
        val queue = InputEventQueue<Int>(capacity = 1)
        assertTrue(queue.trySendTransient(1))
        assertFalse(queue.trySendTransient(2))

        val important = launch(start = CoroutineStart.UNDISPATCHED) {
            queue.sendImportant(3)
        }
        assertFalse(important.isCompleted)

        assertEquals(1, queue.receive())
        important.join()
        assertEquals(3, queue.receive())
    }

    @Test
    fun importantSendersKeepInvocationOrder() = runTest {
        val queue = InputEventQueue<Int>(capacity = 2)
        val first = launch(start = CoroutineStart.UNDISPATCHED) { queue.sendImportant(1) }
        val second = launch(start = CoroutineStart.UNDISPATCHED) { queue.sendImportant(2) }
        val third = launch(start = CoroutineStart.UNDISPATCHED) { queue.sendImportant(3) }

        assertEquals(1, queue.receive())
        assertEquals(2, queue.receive())
        assertEquals(3, queue.receive())
        first.join()
        second.join()
        third.join()
    }
}
