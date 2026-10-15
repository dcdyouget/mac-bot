package bot.mac.mobile.feature.computer

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ComputerTakeoverLifecycleTest {
    @Test
    fun themeOrConnectionRecreationDoesNotNavigateAway() {
        assertFalse(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "computer", "bot-1", TakeoverOwner("host-1", "bot-1"), "host-1"))
    }

    @Test
    fun takeoverIsReleasedWhenLeavingComputerScreen() {
        assertTrue(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "chat", "chat-1", TakeoverOwner("host-1", "bot-1"), "host-1"))
    }

    @Test
    fun pageExitDoesNotReleaseWhenThisScreenDoesNotOwnTakeover() {
        assertFalse(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "chat", "chat-1", null, "host-1"))
        assertTrue(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "computer", "bot-2", TakeoverOwner("host-1", "bot-1"), "host-1"))
        assertTrue(shouldReleaseTakeoverOnNavigation("computer", "bot-1", "computer", "bot-1", TakeoverOwner("host-1", "bot-1"), "host-2"))
    }

    @Test
    fun takeoverOwnerIdentitySurvivesRouteResetAndHostChange() {
        val owner = TakeoverOwner("host-1", "bot-1")
        assertTrue(shouldReleaseTakeoverOnHostChange(owner, "host-2"))
        assertFalse(shouldReleaseTakeoverOnHostChange(owner, "host-1"))
        assertFalse(shouldReleaseTakeoverOnHostChange(owner, null))
        assertFalse(shouldReleaseTakeoverOnHostChange(null, "host-2"))
    }
}
