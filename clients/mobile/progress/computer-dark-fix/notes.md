# Computer dark mode and Activity recreation

## Finding

The batch-03 `computer-dark.png` zero-frame state was not reproducible. On the pre-fix APK, the Computer screen continued receiving frames through light → dark → light (`computer-dark-before.png`, 58 FPS; `computer-light-after-toggle.png`, 52 FPS). The batch-03 capture showed the initial “正在连接画面…” state, consistent with a transient connection handshake. No frame receive, decode, or ack code was changed.

A separate lifecycle defect was reproducible: when the screen initiated `takeover.start`, `LaunchedEffect` cleanup sent `takeover.release` during Activity recreation, returning control to the Bot during a system theme change.

## Change

- Read the owning Activity's live `isChangingConfigurations` state through an Android platform expect/actual.
- During configuration recreation, stop the old `ScreenConnection` without releasing takeover.
- Save the locally owned takeover Host ID with `rememberSaveable`, so a later real exit still releases takeover.
- Add `ComputerTakeoverLifecycleTest` for recreation, normal exit, and mismatched or missing Host cases.

## Verification

- `ANDROID_HOME=/Users/gongshaojie/Library/Android/sdk ./gradlew :shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest :androidApp:assembleDebug` — BUILD SUCCESSFUL (84 tasks).
- Independent API 36 AVD `emulator-5558`, mock `10.0.2.2:17963`, password `dev`: `takeover.start` RPC returned `{"ok":true,"result":{}}`. Through light → dark → light, mock JPEG frames remained visible and the UI continued to show “你正在操作（Bot 已暂停）” / “交还给 Bot”. Screenshots are `computer-dark-after-fix.png` and `computer-light-after-toggle-fix.png`. `computer-light-after-fix.png` shows a live light-mode frame at 60 FPS.
- Final source including `rememberSaveable` passed all three Gradle gates and APK assembly. The final APK was installed on a clean API 36 AVD (`emulator-5582`) with a fresh mock at `10.0.2.2:17965` (password `dev`), configured through the app's Host form. The UI `接管并处理` action showed the owned takeover state. Across system light → dark → light changes, the final UI retained “你正在操作（Bot 已暂停）”; the preview remained live and its mock frame content advanced. A separate authenticated `/ws/screen?bot_id=bot_product` observer received state `driver=user` and ACKed 2,524 frames during a 3-second sample. After tapping the Computer page Back control, the app returned to Main Bot; the same observer then reported `driver=bot` and received/ACKed 1,276 frames during a 1-second sample. This verifies takeover survives configuration recreation and normal page exit releases it. `git diff --check` passed; no commit was made.

## Screenshots

- `computer-light-after-fix.png` — light mode, live frame, 60 FPS.
- `computer-dark-before.png` — pre-fix dark mode, live frame, 58 FPS.
- `computer-light-after-toggle.png` — pre-fix light mode after theme change; live frame, but takeover returned to Bot operation.
- `computer-dark-takeover-before-fix.png` — pre-fix dark takeover state.
- `computer-dark-after-fix.png` — dark mode after change; live frame and takeover active.
- `computer-light-after-toggle-fix.png` — light mode after change; live frame and takeover active.
- `final-takeover-start.png` — final APK, takeover initiated through the Computer UI.
- `final-takeover-dark.png` — final APK after changing to dark mode; takeover retained and screen frame visible.
- `final-takeover-light-again.png` — final APK after returning to light mode; takeover retained and a later mock frame visible.
- `final-after-computer-exit.png` — final APK returned to Main Bot after leaving the Computer screen.

## Follow-up after independent verifier rejection

The independent verifier's report in the main worktree reproduced the failure twice on AVD `emulator-5592`: after Android theme recreation, `driver=user` became `driver=bot`. The previous workaround sampled `Activity.isChangingConfigurations` when `ScreenConnection`'s `LaunchedEffect` ended. That effect can end for reasons other than a real page exit, and the sampled Activity flag is timing dependent; connection cleanup must not own takeover release.

The follow-up patch moves the owner Host ID into App-level `rememberSaveable` state. Ending or replacing a screen connection now only stops the stream. App `navigate` and `back` release takeover on the owner Host before changing routes away from Computer or changing to another Bot. This covers the top return button, Android Back, routes opened from events/deep links, and Host navigation, while same-route activity/theme recreation preserves ownership. The takeover owner is recorded before the start RPC returns, so a route exit racing the RPC still has the owner needed for release. `ComputerTakeoverLifecycleTest` now covers route recreation versus route departure, another Bot, and missing ownership.

### Verification

- Final worktree source passed `:shared:commonTest`, `:shared:androidUnitTest`, `:androidApp:testDebugUnitTest`, and `:androidApp:assembleDebug` — `BUILD SUCCESSFUL`, 84 tasks. `git diff --check` passed.
- A/B baseline: installed the verifier's exact APK SHA-256 `ea6cac3c664af6f0a7b56643eddbeedd2ea6964757f757dd1e5a596aeec94e2e` on independent AVD `emulator-5594`. In my run it remained `driver=user` through two system-theme toggles, despite an Activity relaunch in logcat. This does not reproduce the verifier's failure and is recorded as an environment difference, not as a baseline pass.
- Latest final patch APK SHA-256 `9863a4f88ad33ff3b7342b0899b57c783ca0ceb37e48c2503e063af43525e06f` on the same API 36 AVD and isolated mock `10.0.2.2:17967`: UI `接管并处理` started takeover. On this exact APK, a dark theme change retained `driver=user` and the observer ACKed 1,340 frames in 0.4 seconds; opening `macbot://chat/chat_main` then navigated to Main Bot and the observer reported `driver=bot` with 2,926 ACKs in 0.4 seconds. The immediately preceding patch APK, differing only by reading the active Host live during navigation, also passed repeated dark → light → dark changes (`driver=user`, 618 / 1,903 / 893 ACKs in separate 0.4-second samples) and Android Back release (`driver=bot`, 873 ACKs in 0.5 seconds). No commit was made.

### Follow-up screenshots

- `owner-fix-baseline-dark-repeat.png` — exact baseline APK after repeated theme changes; still shows takeover active in this AVD, documenting the local non-reproduction.
- `owner-fix-final-takeover.png`, `owner-fix-final-dark-1.png`, `owner-fix-final-light.png`, `owner-fix-final-dark-2.png` — final patch APK after UI takeover and successive theme changes.
- `owner-fix-final-after-back.png` — final patch APK after system Back left Computer.
- `owner-fix-final-before-deeplink.png`, `owner-fix-final-after-deeplink.png` — final patch APK before and after chat deep-link navigation away from Computer.
