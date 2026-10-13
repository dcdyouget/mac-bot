package bot.mac.mobile.core.ui

import kotlin.test.Test
import kotlin.test.assertEquals

class BotDisplayNameTest {
    @Test
    fun mainChatUsesServerTitleOrNameBeforeTheLocalizedDefault() {
        assertEquals("秘书长", mainBotChatTitle(null, null, "秘书长"))
        assertEquals("秘书长 - 私聊", mainBotChatTitle("秘书长 - 私聊", "用户改名", "秘书长"))
        assertEquals("用户改名", mainBotChatTitle("", "用户改名", "秘书长"))
    }

    @Test
    fun mainBotFallbackDoesNotReplaceANameFromTheServer() {
        assertEquals("我的秘书", botDisplayName("我的秘书", "bot_main", true, "秘书长"))
        assertEquals("秘书长", botDisplayName(" ", "bot_main", true, "秘书长"))
        assertEquals("bot_worker", botDisplayName("", "bot_worker", false, "秘书长"))
    }
}
