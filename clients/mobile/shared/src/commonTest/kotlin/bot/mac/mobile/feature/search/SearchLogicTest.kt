package bot.mac.mobile.feature.search

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class SearchLogicTest {
    @Test
    fun onlyProtocolSearchKindsAreForwarded() {
        assertEquals("message", searchKindParameter("message"))
        assertEquals("routine", searchKindParameter("routine"))
        assertNull(searchKindParameter("unknown"))
    }
}
