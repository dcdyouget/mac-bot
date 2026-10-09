package bot.mac.mobile

import android.Manifest
import android.app.Application
import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class NotificationAndroidBackendTest {
    private lateinit var context: Context
    private lateinit var manager: NotificationManager

    @Before
    fun setUp() {
        context = RuntimeEnvironment.getApplication()
        manager = context.getSystemService(NotificationManager::class.java)
        manager.cancelAll()
        context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE).edit().clear().commit()
        Shadows.shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        MacBotNotifications.ensureChannels(context)
    }

    @Test
    fun backendMapsActiveNotificationsProtectsSystemEntriesAndPersistsShownDelivery() = runBlocking {
        seedFiftyNotifications()
        val backend = AndroidNotificationDeliveryBackend(context) { true }
        val activeBefore = backend.active()
        val foreground = activeBefore.first { it.identity.id == FOREGROUND_ID }
        val summary = activeBefore.first { it.identity.id == SUMMARY_ID }
        assertEquals(50, activeBefore.size)
        assertEquals("foreground", foreground.identity.tag)
        assertEquals("summary", summary.identity.tag)
        assertTrue(foreground.`protected`)
        assertTrue(summary.`protected`)

        val request = completedRequest("assignment-android-1", seq = 91)
        val store = AndroidNotificationDeliveryStore(context)
        val coordinator = NotificationDeliveryCoordinator(store, backend, pause = {})
        coordinator.enqueue(request)

        assertFalse(coordinator.deliverPending())
        val activeAfter = backend.active()
        assertEquals(40, activeAfter.size)
        assertTrue(activeAfter.any { it.identity.id == FOREGROUND_ID })
        assertTrue(activeAfter.any { it.identity.id == SUMMARY_ID })

        val shown = manager.activeNotifications.firstOrNull {
            it.tag == request.identity.tag && it.id == request.identity.id
        }
        assertNotNull(shown)
        assertEquals(request.ledgerKey, shown!!.notification.extras.getString(MacBotNotifications.DELIVERY_KEY))
        assertEquals(request.marker, shown.notification.extras.getString(MacBotNotifications.DELIVERY_MARKER))
        val prefs = context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE)
        assertEquals(request.marker, prefs.getString(request.ledgerKey, null))
        assertEquals("[]", prefs.getString(PENDING_KEY, null))
    }

    @Test
    fun pendingJsonInSharedPreferencesSurvivesStoreAndCoordinatorRestart() = runBlocking {
        val request = completedRequest("assignment-recovery", seq = 92)
        val store = AndroidNotificationDeliveryStore(context)
        val blockedBackend = AndroidNotificationDeliveryBackend(context) { false }
        val first = NotificationDeliveryCoordinator(store, blockedBackend, pause = {})
        first.enqueue(request)
        assertTrue(first.deliverPending())

        val prefs = context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE)
        val serialized = prefs.getString(PENDING_KEY, null)
        assertNotNull(serialized)
        assertTrue(serialized!!.contains("assignment-recovery"))
        assertEquals(null, prefs.getString(request.ledgerKey, null))

        val recovered = NotificationDeliveryCoordinator(
            AndroidNotificationDeliveryStore(context),
            AndroidNotificationDeliveryBackend(context) { true },
            pause = {},
        )
        assertFalse(recovered.deliverPending())
        assertEquals(request.marker, prefs.getString(request.ledgerKey, null))
        assertEquals("[]", prefs.getString(PENDING_KEY, null))
        assertTrue(manager.activeNotifications.any { it.id == request.identity.id && it.tag == request.identity.tag })
    }

    @Test
    fun disabledMessageChannelKeepsPendingWhileCompletedChannelDeliversAndRecovers() = runBlocking {
        val messageChannel = requireNotNull(manager.getNotificationChannel(MacBotNotifications.CHANNEL_MESSAGES))
        messageChannel.setImportance(NotificationManager.IMPORTANCE_NONE)
        assertEquals(NotificationManager.IMPORTANCE_NONE, messageChannel.importance)

        val message = messageRequest("message-channel-disabled", seq = 93)
        val completed = completedRequest("assignment-channel-enabled", seq = 94)
        val store = AndroidNotificationDeliveryStore(context)
        val backend = AndroidNotificationDeliveryBackend(context) { true }
        val coordinator = NotificationDeliveryCoordinator(store, backend, pause = {})
        coordinator.enqueue(message)
        coordinator.enqueue(completed)

        assertTrue(coordinator.deliverPending())
        assertEquals(null, store.marker(message.ledgerKey))
        assertEquals(completed.marker, store.marker(completed.ledgerKey))
        assertTrue(manager.activeNotifications.any { it.id == completed.identity.id && it.tag == completed.identity.tag })
        assertFalse(manager.activeNotifications.any { it.id == message.identity.id && it.tag == message.identity.tag })

        // Robolectric permits changing the channel object to simulate the user
        // re-enabling it; the durable queue then retries the message.
        messageChannel.setImportance(NotificationManager.IMPORTANCE_DEFAULT)
        assertFalse(coordinator.deliverPending())
        assertEquals(message.marker, store.marker(message.ledgerKey))
        assertTrue(manager.activeNotifications.any { it.id == message.identity.id && it.tag == message.identity.tag })
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
        hostId = "host-android",
        id = id,
        title = "已完成",
        text = "完成 $id",
        seq = seq,
        projectId = "project-android",
        deepLinkKind = "review",
        deepLinkId = "project-android",
    )

    private fun messageRequest(id: String, seq: Long) = PendingNotification(
        kind = NotificationKind.MESSAGE,
        hostId = "host-android",
        id = id,
        title = "新消息",
        text = id,
        seq = seq,
        chatId = "chat-android",
        deepLinkKind = "chat",
        deepLinkId = "chat-android",
    )

    private companion object {
        const val LEDGER_PREFS = "macbot_notification_ledger"
        const val PENDING_KEY = "_pending_deliveries"
        const val FOREGROUND_ID = 100
        const val SUMMARY_ID = 101
    }
}
