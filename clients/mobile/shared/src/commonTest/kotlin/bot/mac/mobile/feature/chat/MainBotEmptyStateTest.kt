package bot.mac.mobile.feature.chat

import kotlin.test.Test
import kotlin.test.assertEquals

class MainBotEmptyStateTest {
    @Test
    fun selectingSuggestionFillsDraftBeforeFocusingComposerWithoutSending() {
        val events = mutableListOf<String>()
        var draft = ""
        val suggestion = "帮我给 App 加个登录"

        applyMainBotSuggestion(
            suggestion,
            setDraft = { draft = it; events += "draft:$it" },
            requestFocus = { events += "focus" },
        )

        assertEquals(suggestion, draft)
        assertEquals(listOf("draft:$suggestion", "focus"), events)
    }
}
