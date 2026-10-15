package bot.mac.mobile.feature.connect

/** Whether a route and its remembered screen state belong to the previous Host. */
internal fun shouldResetHostScopedRoute(previousHostId: String?, nextHostId: String?): Boolean =
    previousHostId != null && nextHostId != null && previousHostId != nextHostId
