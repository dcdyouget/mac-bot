package bot.mac.mobile.feature.connect

import bot.mac.mobile.core.network.ConnectionStatus

internal fun canUseOnboardingHost(
    requestedHostId: String,
    activeHostId: String?,
    status: ConnectionStatus,
): Boolean = requestedHostId == activeHostId && status == ConnectionStatus.CONNECTED

internal fun isCurrentOnboardingHost(requestedHostId: String, activeHostId: String?): Boolean =
    requestedHostId == activeHostId
