package bot.mac.mobile.feature.chat

import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.chat_time_date
import bot.mac.mobile.resources.chat_time_date_year
import bot.mac.mobile.resources.chat_time_full
import bot.mac.mobile.resources.chat_time_today
import kotlinx.coroutines.test.runTest
import org.jetbrains.compose.resources.getString
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import kotlin.test.assertEquals

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class ChatMessageTimeResourceTest {
    @Test
    fun todayMessageTimeUsesSupportedPlaceholdersAndPadsSingleDigits() = runTest {
        assertEquals(
            "09:05",
            getString(Res.string.chat_time_today, padChatTimePart(9), padChatTimePart(5)),
        )
        assertEquals(
            "00:00",
            getString(Res.string.chat_time_today, padChatTimePart(0), padChatTimePart(0)),
        )
        assertEquals(
            "23:59",
            getString(Res.string.chat_time_today, padChatTimePart(23), padChatTimePart(59)),
        )
    }

    @Test
    fun historicalDateAndFullMessageTimeKeepTheirZeroPadding() = runTest {
        assertEquals("03/07", getString(Res.string.chat_time_date, padChatTimePart(3), padChatTimePart(7)))
        assertEquals("2026/03/07", getString(Res.string.chat_time_date_year, 2026, padChatTimePart(3), padChatTimePart(7)))
        assertEquals(
            "2026/03/07 09:05:02",
            getString(
                Res.string.chat_time_full,
                2026,
                padChatTimePart(3),
                padChatTimePart(7),
                padChatTimePart(9),
                padChatTimePart(5),
                padChatTimePart(2),
            ),
        )
    }
}
