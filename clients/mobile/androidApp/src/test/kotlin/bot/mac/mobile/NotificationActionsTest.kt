package bot.mac.mobile

import android.app.Notification
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.annotation.Config
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class NotificationActionsTest {
    private lateinit var context: Context

    @Before
    fun setUp() {
        context = org.robolectric.RuntimeEnvironment.getApplication()
    }

    @Test
    fun approvalNotificationHasThreeHostScopedActionsAndDeepLink() {
        val notification = buildNotification(
            context,
            PendingNotification(
                kind = NotificationKind.APPROVAL,
                hostId = "host-a",
                id = "approval-1",
                title = "需要确认",
                text = "确认操作",
                seq = 17L,
                deepLinkKind = "approval",
                deepLinkId = "approval-1",
            ),
        )

        val contentIntent = savedIntent(notification.contentIntent)
        assertEquals("macbot", contentIntent.data?.scheme)
        assertEquals("approval", contentIntent.data?.host)
        assertEquals("/approval-1", contentIntent.data?.path)
        assertEquals("host-a", contentIntent.data?.getQueryParameter("host_id"))

        val actions = notification.actions.orEmpty()
        assertEquals(3, actions.size)
        assertEquals(
            setOf("allow_once", "always_allow", "deny"),
            actions.map { savedIntent(it.actionIntent).getStringExtra("approval_action") }.toSet(),
        )
        actions.forEach { action ->
            val intent = savedIntent(action.actionIntent)
            assertEquals("host-a", intent.getStringExtra("host_id"))
            assertEquals("approval-1", intent.getStringExtra("approval_id"))
            assertEquals(ApprovalActionReceiver::class.java.name, intent.component?.className)
            assertNotNull("broadcast intent must have a unique data URI", intent.data)
        }
    }

    @Test
    fun completedProjectNotificationHasReviewConfirmationAction() {
        val notification = buildNotification(
            context,
            PendingNotification(
                kind = NotificationKind.COMPLETED,
                hostId = "host-a",
                id = "assignment-1",
                title = "已完成",
                text = "工作已完成",
                projectId = "project-1",
                deepLinkKind = "review",
                deepLinkId = "project-1",
            ),
        )

        assertEquals(1, notification.actions.orEmpty().size)
        val intent = savedIntent(notification.actions!![0].actionIntent)
        assertEquals("host-a", intent.getStringExtra("host_id"))
        assertEquals("project-1", intent.getStringExtra("approval_id"))
        assertEquals("confirm_done", intent.getStringExtra("approval_action"))
        assertEquals(ApprovalActionReceiver::class.java.name, intent.component?.className)
        assertNotNull("review broadcast intent must have a unique data URI", intent.data)
    }

    @Test
    fun sameNotificationIdOnDifferentHostsHasDistinctDeepLinksAndPendingIntents() {
        val first = approvalNotification("host-a")
        val second = approvalNotification("host-b")

        val firstContent = first.contentIntent!!
        val secondContent = second.contentIntent!!
        assertNotEquals(savedIntent(firstContent).data, savedIntent(secondContent).data)
        assertNotEquals(Shadows.shadowOf(firstContent).requestCode, Shadows.shadowOf(secondContent).requestCode)

        val firstAction = savedIntent(first.actions!![0].actionIntent)
        val secondAction = savedIntent(second.actions!![0].actionIntent)
        assertNotEquals(firstAction.data, secondAction.data)
        assertNotEquals(
            Shadows.shadowOf(first.actions!![0].actionIntent).requestCode,
            Shadows.shadowOf(second.actions!![0].actionIntent).requestCode,
        )
        assertTrue(firstAction.data != null && secondAction.data != null)
    }

    private fun approvalNotification(hostId: String): Notification = buildNotification(
        context,
        PendingNotification(
            kind = NotificationKind.APPROVAL,
            hostId = hostId,
            id = "same-approval",
            title = "需要确认",
            text = "确认操作",
            deepLinkKind = "approval",
            deepLinkId = "same-approval",
        ),
    )

    private fun savedIntent(pendingIntent: PendingIntent?): Intent {
        requireNotNull(pendingIntent)
        return Shadows.shadowOf(pendingIntent).savedIntent
    }
}
