package bot.mac.mobile.core.platform

import bot.mac.mobile.core.state.MobileState

internal data class DeepLinkDestination(
    val kind: DeepLinkKind,
    val resourceId: String,
    val messageId: String?,
)

/** Switches Host only after removing the old Host's route, then resolves the link against target state. */
internal suspend fun resolveDeepLinkNavigation(
    link: MobileDeepLink,
    currentHostId: String?,
    knownHostIds: Set<String>,
    selectHost: suspend (String) -> Unit,
    stateForHost: (String) -> MobileState,
    onHostSwitch: (String) -> Unit,
): DeepLinkDestination? {
    val targetHostId = link.resolveHost(knownHostIds, currentHostId) ?: return null
    if (targetHostId != currentHostId) onHostSwitch(targetHostId)
    selectHost(targetHostId)
    if (!stateForHost(targetHostId).containsDeepLinkResource(link)) return null
    return DeepLinkDestination(link.kind, link.resourceId, link.messageId)
}
