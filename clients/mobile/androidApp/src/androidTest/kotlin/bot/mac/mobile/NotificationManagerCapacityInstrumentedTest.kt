package bot.mac.mobile

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import androidx.core.app.NotificationCompat
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.rule.GrantPermissionRule
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Real NotificationManager capacity check in the opt-in isolated package. */
@RunWith(AndroidJUnit4::class)
class NotificationManagerCapacityInstrumentedTest {
    private lateinit var context: Context
    private lateinit var manager: NotificationManager

    @get:Rule
    val notificationPermission: GrantPermissionRule =
        GrantPermissionRule.grant(Manifest.permission.POST_NOTIFICATIONS)

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        assertEquals("bot.mac.mobile.capacitytest", context.packageName)
        manager = context.getSystemService(NotificationManager::class.java)
        manager.cancelAll()
        context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE).edit().clear().commit()
        MacBotNotifications.ensureChannels(context)
    }

    @After
    fun tearDown() {
        manager.cancelAll()
        context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE).edit().clear().commit()
    }

    @Test
    fun realManagerRetainsProtectedEntriesAndCoordinatorStaysBelowQuota() = runBlocking {
        seedFiftyNotifications()
        val activeBefore = awaitActive { it.size == 50 }
        assertEquals(50, activeBefore.size)
        assertTrue(activeBefore.any { it.id == FOREGROUND_ID })
        assertTrue(activeBefore.any { it.id == SUMMARY_ID })

        val request = completedRequest("assignment-capacity-instrumented", seq = 91)
        val coordinator = NotificationDeliveryCoordinator(
            AndroidNotificationDeliveryStore(context),
            AndroidNotificationDeliveryBackend(context) { true },
            pause = {},
        )
        coordinator.enqueue(request)

        assertFalse(coordinator.deliverPending())
        val activeAfter = awaitActive {
            it.size == 40 && it.any { record ->
                record.tag == request.identity.tag && record.id == request.identity.id
            }
        }
        assertEquals(40, activeAfter.size)
        assertTrue(activeAfter.any { it.id == FOREGROUND_ID })
        assertTrue(activeAfter.any { it.id == SUMMARY_ID })

        val shown = activeAfter.firstOrNull {
            it.tag == request.identity.tag && it.id == request.identity.id
        }
        assertNotNull(shown)
        assertEquals(request.ledgerKey, shown!!.notification.extras.getString(MacBotNotifications.DELIVERY_KEY))
        assertEquals(request.marker, shown.notification.extras.getString(MacBotNotifications.DELIVERY_MARKER))
        val prefs = context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE)
        assertEquals(request.marker, prefs.getString(request.ledgerKey, null))
        assertEquals("[]", prefs.getString(PENDING_KEY, null))
    }

    private fun awaitActive(predicate: (List<android.service.notification.StatusBarNotification>) -> Boolean): List<android.service.notification.StatusBarNotification> {
        repeat(50) {
            val active = manager.activeNotifications.toList()
            if (predicate(active)) return active
            Thread.sleep(100)
        }
        return manager.activeNotifications.toList().also { assertTrue(predicate(it)) }
    }

    private fun seedFiftyNotifications() {
        repeat(48) { index ->
            manager.notify(
                "seed-$index",
                index,
                NotificationCompat.Builder(context, MacBotNotifications.CHANNEL_MESSAGES)
                    .setSmallIcon(android.R.drawable.sym_action_chat)
                    .setContentTitle("seed")
                    .setContentText(index.toString())
                    .build(),
            )
        }
        val foreground = NotificationCompat.Builder(context, MacBotNotifications.CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setOngoing(true)
            .build()
            .apply { flags = flags or Notification.FLAG_FOREGROUND_SERVICE }
        manager.notify("foreground", FOREGROUND_ID, foreground)
        val summary = NotificationCompat.Builder(context, MacBotNotifications.CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.sym_action_chat)
            .setGroup("seed-group")
            .setGroupSummary(true)
            .build()
        manager.notify("summary", SUMMARY_ID, summary)
    }

    private fun completedRequest(id: String, seq: Long) = PendingNotification(
        kind = NotificationKind.COMPLETED,
        hostId = "host-capacity-instrumented",
        id = id,
        title = "已完成",
        text = "完成 $id",
        seq = seq,
        projectId = "project-capacity-instrumented",
        deepLinkKind = "review",
        deepLinkId = "project-capacity-instrumented",
    )

    private companion object {
        const val LEDGER_PREFS = "macbot_notification_ledger"
        const val PENDING_KEY = "_pending_deliveries"
        const val FOREGROUND_ID = 100
        const val SUMMARY_ID = 101
    }
}
