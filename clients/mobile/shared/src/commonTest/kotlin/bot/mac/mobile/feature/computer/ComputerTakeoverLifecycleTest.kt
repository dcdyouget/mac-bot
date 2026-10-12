package bot.mac.mobile.feature.computer

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ComputerTakeoverLifecycleTest {
    @Test
    fun themeOrConnectionRecreationDoesNotNavigateAway() {
        assertFalse(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "computer", "bot-1", "host-1", "host-1"))
    }

    @Test
    fun takeoverIsReleasedWhenLeavingComputerScreen() {
        assertTrue(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "chat", "chat-1", "host-1", "host-1"))
    }

    @Test
    fun pageExitDoesNotReleaseWhenThisScreenDoesNotOwnTakeover() {
        assertFalse(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "chat", "chat-1", null, "host-1"))
        assertTrue(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "computer", "bot-2", "host-1", "host-1"))
        assertTrue(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "computer", "bot-1", "host-1", "host-2"))
    }

    @Test
    fun directHostChangeReleasesComputerTakeoverWithoutRouteChange() {
        assertTrue(shouldReleaseTakeoverOnHostChange("computer", "host-1", "host-2"))
        assertFalse(shouldReleaseTakeoverOnHostChange("computer", "host-1", "host-1"))
        assertFalse(shouldReleaseTakeoverOnHostChange("computer", "host-1", null))
        assertFalse(shouldReleaseTakeoverOnHostChange("chat", "host-1", "host-2"))
        assertFalse(shouldReleaseTakeoverOnHostChange("computer", null, "host-2"))
    }
}
