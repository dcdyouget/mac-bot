package bot.mac.mobile

import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Test

class NotificationMessageRoutingTest {
    @Test
    fun privateMessagesRemainEnabledWithoutGroupPreference() {
        assertEquals(
            MessageNotificationRoute.PRIVATE_MESSAGE,
            classifyMessageNotification(botMessage(), "bot_dm", notifyAllGroupMessages = false),
        )
    }

    @Test
    fun groupMessagesRequireTheIndependentPreference() {
        val message = botMessage()

        assertEquals(
            MessageNotificationRoute.IGNORE,
            classifyMessageNotification(message, "project", notifyAllGroupMessages = false),
        )
        assertEquals(
            MessageNotificationRoute.GROUP_MESSAGE,
            classifyMessageNotification(message, "project", notifyAllGroupMessages = true),
        )
    }

    @Test
    fun mentionsAndTakeoversRemainNeedsYouEvenWhenGroupMessagesAreOff() {
        val mention = buildJsonObject {
            put("sender", botSender())
            put("mentions", buildJsonArray { add(buildJsonObject { put("kind", "user") }) })
        }
        val takeover = buildJsonObject {
            put("sender", botSender())
            put("blocks", buildJsonArray {
                add(buildJsonObject { put("type", "takeover_request"); put("state", "pending") })
            })
        }

        assertEquals(MessageNotificationRoute.NEEDS_YOU, classifyMessageNotification(mention, "project", false))
        assertEquals(MessageNotificationRoute.NEEDS_YOU, classifyMessageNotification(takeover, "project", false))
    }

    @Test
    fun ownMessagesAndIncompleteStreamingUpdatesDoNotNotify() {
        val ownMention = buildJsonObject {
            put("sender", buildJsonObject { put("kind", "user") })
            put("mentions", buildJsonArray { add(buildJsonObject { put("kind", "user") }) })
        }
        val streaming = buildJsonObject {
            put("sender", botSender())
            put("streaming", true)
        }

        assertEquals(MessageNotificationRoute.IGNORE, classifyMessageNotification(ownMention, "project", true))
        assertEquals(MessageNotificationRoute.IGNORE, classifyMessageNotification(streaming, "bot_dm", false))
    }

    private fun botMessage() = buildJsonObject { put("sender", botSender()) }

    private fun botSender() = buildJsonObject {
        put("kind", "bot")
        put("bot_id", "bot_demo")
    }
}
