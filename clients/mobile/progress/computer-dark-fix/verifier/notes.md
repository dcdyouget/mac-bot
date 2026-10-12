# Independent verification: Computer takeover across theme recreation

**Result: failed acceptance.** Gradle gates pass, and normal navigation away from Computer releases takeover. However, changing the Android system theme while takeover is active changes the server driver from `user` back to `bot`; the UI returns to “Bot 操作中”. This reproduces the lifecycle bug the patch is intended to prevent.

## Build and tests

From `clients/mobile/`:

```sh
ANDROID_HOME=/Users/gongshaojie/Library/Android/sdk ./gradlew :shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest :androidApp:assembleDebug
```

Result: `BUILD SUCCESSFUL in 42s`, 84 tasks (13 executed, 71 up-to-date). This includes `ComputerTakeoverLifecycleTest`'s configuration recreation, normal exit, and mismatched/missing Host cases. Protocol-generated model/fixture changes were present in the main worktree; no task failed or reported an error attributable to them.

Final APK SHA-256:

```text
ea6cac3c664af6f0a7b56643eddbeedd2ea6964757f757dd1e5a596aeec94e2e
```

## UI and server check

- Device: Android API 36, dedicated emulator process `emulator-5592` using the `macbot_api36` image. It was separate from the already-running `emulator-5584`; only `emulator-5592` was operated and then shut down.
- Mock: `10.0.2.2:17963`, password `dev`, isolated home `/tmp/macbot-computer-dark-verify-home`.
- Cleared app data, configured this mock Host in the APK, opened Bot 管理 → 编码 → 打开屏幕, and tapped 接管并处理.
- The UI showed “你正在操作（Bot 已暂停）”; an independent authenticated `/ws/screen?bot_id=bot_code` observer reported `driver=user`.
- `adb shell cmd uimode night yes` recreated the screen. After that change, the UI showed “Bot 操作中” and the observer reported `driver=bot`. The image stream remained visible, so the regression is ownership release during recreation, not frame connection loss.
- Repeated the flow after returning to light: takeover again reported `user`, and a second system theme change again reported `bot` and showed “Bot 操作中”.
- In dark mode, tapping 接管并处理 restored “你正在操作（Bot 已暂停）”. Leaving Computer through its Back control returned to the Bot detail page; the observer reported `driver=bot`, confirming normal exit release.

## Screenshots

- `10-computer-light-before-takeover.png` — light mode before taking over; live mock frame.
- `11-computer-light-takeover.png` — light mode after take over; warning outline and paused Bot state.
- `12-computer-dark-takeover.png` — after switching to dark; driver/UI reverted to Bot control, despite a visible frame.
- `13-computer-light-after-theme.png`, `14-computer-light-retake.png` — return to light and take over again.
- `15-computer-dark-retake-theme.png` — second theme change also returned to Bot control.
- `16-computer-dark-before-exit.png`, `17-after-computer-exit.png` — take over again in dark, then leave Computer.

The evidence disproves the claimed UI behavior in `../notes.md` for the final APK under this independent run. No product source, shared file, or commit was changed by this verifier.
