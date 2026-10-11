package bot.mac.mobile.core.ui

import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import bot.mac.mobile.core.design.LocalSystemAnimationsEnabled

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MotionAwareLinearProgressIndicator(modifier: Modifier = Modifier) {
    if (LocalSystemAnimationsEnabled.current) {
        LinearProgressIndicator(modifier = modifier)
    } else {
        LinearProgressIndicator(progress = { 0.3f }, modifier = modifier)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MotionAwareCircularProgressIndicator(modifier: Modifier = Modifier) {
    if (LocalSystemAnimationsEnabled.current) {
        CircularProgressIndicator(modifier = modifier)
    } else {
        CircularProgressIndicator(progress = { 0.3f }, modifier = modifier)
    }
}
