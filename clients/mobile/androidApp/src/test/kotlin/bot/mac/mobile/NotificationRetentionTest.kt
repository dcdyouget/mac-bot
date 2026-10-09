package bot.mac.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationRetentionTest {
    @Test
    fun fullFiftyEntryStoreEvictsSpaceForEachNotificationClass() {
        val active = (0 until 50).map { index ->
            RetainedNotification(NotificationIdentity("host-a", index), postedAt = index.toLong())
        }

        listOf(
            NotificationIdentity("needs-you", 100),
            NotificationIdentity("completed", 101),
            NotificationIdentity("messages", 102),
        ).forEach { incoming ->
            assertEquals(11, NotificationRetention.evictions(active, incoming).size)
        }
    }

    @Test
    fun foregroundAndSummaryNotificationsAreProtected() {
        val foreground = RetainedNotification(
            NotificationIdentity(null, 100), postedAt = 0, priority = -100, `protected` = true,
        )
        val summary = RetainedNotification(
            NotificationIdentity("summary", 1), postedAt = 1, priority = -100, `protected` = true,
        )
        val active = listOf(foreground, summary) + (0 until 48).map { index ->
            RetainedNotification(NotificationIdentity("messages", index), postedAt = (index + 2).toLong())
        }

        val evictions = NotificationRetention.evictions(active, NotificationIdentity("messages", 100))

        assertEquals(11, evictions.size)
        assertFalse(foreground.identity in evictions)
        assertFalse(summary.identity in evictions)
    }

    @Test
    fun allHighPriorityEntriesStillGuaranteeSpaceForIncoming() {
        val active = (0 until 50).map { index ->
            RetainedNotification(NotificationIdentity("messages", index), postedAt = index.toLong(), priority = 100)
        }

        val evictions = NotificationRetention.evictions(active, NotificationIdentity("messages", 50))

        assertEquals(11, evictions.size)
        assertEquals((0 until 11).map { NotificationIdentity("messages", it) }, evictions)
    }

    @Test
    fun existingIncomingIdentityIsRetainedAndOnlyFreesExistingSlots() {
        val incoming = NotificationIdentity("messages", 7)
        val active = (0 until 50).map { index ->
            RetainedNotification(
                identity = if (index == 49) incoming else NotificationIdentity("messages", index),
                postedAt = index.toLong(),
            )
        }

        val evictions = NotificationRetention.evictions(active, incoming)

        assertEquals(10, evictions.size)
        assertFalse(incoming in evictions)
    }

    @Test
    fun tagAndIdDistinguishHostsEvenWhenIdentityHashesCollide() {
        val hostA = NotificationIdentity("Aa", 7)
        val hostB = NotificationIdentity("BB", 7)
        assertEquals(hostA.hashCode(), hostB.hashCode())
        assertNotEquals(hostA, hostB)

        val active = listOf(RetainedNotification(hostA, postedAt = 0)).plus(
            (0 until 39).map { index ->
                RetainedNotification(NotificationIdentity("messages", index), postedAt = (index + 1).toLong())
            },
        )

        assertEquals(listOf(hostA), NotificationRetention.evictions(active, hostB))
    }

    @Test
    fun underLimitNeedsNoEvictionIncludingAnIncomingNewIdentity() {
        val active = (0 until 39).map { index ->
            RetainedNotification(NotificationIdentity("messages", index), postedAt = index.toLong())
        }

        assertTrue(NotificationRetention.evictions(active, NotificationIdentity("messages", 39)).isEmpty())
    }

    @Test
    fun equalTimestampsUseStableFairOrderAfterPriorityAndAge() {
        val first = RetainedNotification(NotificationIdentity("messages", 1), postedAt = 100)
        val second = RetainedNotification(NotificationIdentity("messages", 2), postedAt = 100)
        val older = RetainedNotification(NotificationIdentity("messages", 3), postedAt = 99)
        val newer = RetainedNotification(NotificationIdentity("messages", 4), postedAt = 101)
        val highPriority = RetainedNotification(NotificationIdentity("messages", 5), postedAt = 0, priority = 1)
        val fillers = (0 until 38).map { index ->
            RetainedNotification(NotificationIdentity("messages", 100 + index), postedAt = (200 + index).toLong(), priority = 2)
        }

        val evictions = NotificationRetention.evictions(
            listOf(first, second, older, newer, highPriority) + fillers,
            NotificationIdentity("messages", 999),
        )

        assertEquals(4, evictions.size)
        assertEquals(listOf(older.identity, first.identity, second.identity, newer.identity), evictions)
    }
}
