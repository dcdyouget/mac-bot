package bot.mac.mobile.core.platform

import android.app.Activity
import android.content.pm.ActivityInfo
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext

private object LandscapeOrientationState {
    var original: Int? = null
}

@Composable
actual fun LandscapeScreen(enabled: Boolean) {
    val context = LocalContext.current
    val activity = context as? Activity ?: return
    val originalOrientation = remember(activity) {
        LandscapeOrientationState.original ?: activity.requestedOrientation
    }
    DisposableEffect(activity, enabled) {
        if (enabled) {
            if (LandscapeOrientationState.original == null) LandscapeOrientationState.original = originalOrientation
            activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        } else {
            activity.requestedOrientation = LandscapeOrientationState.original ?: originalOrientation
            LandscapeOrientationState.original = null
        }
        onDispose {
            if (enabled && !activity.isChangingConfigurations) {
                activity.requestedOrientation = LandscapeOrientationState.original ?: originalOrientation
                LandscapeOrientationState.original = null
            }
        }
    }
}
