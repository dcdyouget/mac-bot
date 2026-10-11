package bot.mac.mobile.feature.connect

import bot.mac.mobile.core.network.ConnectionStatus
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class OnboardingTemplateStateTest {
    @Test
    fun templateFlowRequiresItsOriginalHostToBeActiveAndConnected() {
        assertTrue(canUseOnboardingHost("host-a", "host-a", ConnectionStatus.CONNECTED))
        assertFalse(canUseOnboardingHost("host-a", "host-b", ConnectionStatus.CONNECTED))
        assertFalse(canUseOnboardingHost("host-a", "host-a", ConnectionStatus.RECONNECTING))
        assertFalse(canUseOnboardingHost("host-a", null, ConnectionStatus.CONNECTED))
    }

    @Test
    fun asyncTemplateResultsAreDiscardedAfterHostSwitch() {
        assertTrue(isCurrentOnboardingHost("host-a", "host-a"))
        assertFalse(isCurrentOnboardingHost("host-a", "host-b"))
        assertFalse(isCurrentOnboardingHost("host-a", null))
    }
}
