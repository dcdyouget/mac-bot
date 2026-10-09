# Mac Bot Android

Android-only Kotlin Multiplatform + Compose Multiplatform 1.12.1. Package/applicationId: `bot.mac.mobile`; launcher: `bot.mac.mobile.MainActivity`. Shared code lives in `shared/src/commonMain` (`core/` and `feature/`); Android APIs are isolated behind platform interfaces in `androidMain` and the `androidApp` shell. No iOS target and no client memory page.

## Build and test

```sh
cd clients/mobile
export JAVA_HOME="$(/usr/libexec/java_home -v 21)"
export ANDROID_HOME="$HOME/Library/Android/sdk"
# Compose 1.12.1 Android artifacts require compile API 37.0; target remains API 36.
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" 'platforms;android-37.0'
./gradlew :androidApp:assembleDebug
./gradlew :shared:commonTest :shared:androidUnitTest
./gradlew :androidApp:connectedDebugAndroidTest
```

Gradle wrapper: 9.8.1; Kotlin: 2.4.21; AGP: 9.4.1; minSdk: 26; targetSdk: 36. AGP 9 runs `commonTest` and `src/androidUnitTest` together in `testAndroidHostTest`; the two aliases above execute that task. Reports: `shared/build/reports/tests/testAndroidHostTest/`.

Protocol fixtures must be generated after server-mac publishes the authoritative contract:

```sh
# From repository root
python3 protocol/kotlin/generate.py --check
```

`--check` is read-only and fails on stale output. Regenerate with
`python3 protocol/kotlin/generate.py`, then rerun the fixture contract tests.

The fixture contract test fails if the corpus is missing; no-fixture builds are not phase completion evidence.

## Emulator deploy

```sh
"$ANDROID_HOME/emulator/emulator" -avd macbot_api36 -gpu host &
"$ANDROID_HOME/platform-tools/adb" wait-for-device
"$ANDROID_HOME/platform-tools/adb" install -r androidApp/build/outputs/apk/debug/androidApp-debug.apk
"$ANDROID_HOME/platform-tools/adb" shell am start -n bot.mac.mobile/.MainActivity
"$ANDROID_HOME/platform-tools/adb" exec-out screencap -p > verification/S0.png
```

Add a Host using `10.0.2.2:7789`, password `dev`, for mock; real host uses `10.0.2.2:7788`. Physical devices can use any reachable IP/domain, including `192.168.31.162:7788`. Each Host accepts multiple ordered addresses, `ws`/`wss`/`http`/`https`, proxy prefixes and IPv6. Passwords are encrypted with Android Keystore; Host snapshots, drafts and replay cursors persist separately per Host.

Debug APK: `androidApp/build/outputs/apk/debug/androidApp-debug.apk`. Signed release APK: `androidApp/build/outputs/apk/release/androidApp-release.apk` (R8 + resource shrinking). Supply `MACBOT_ANDROID_KEYSTORE`, `MACBOT_ANDROID_STORE_PASSWORD`, `MACBOT_ANDROID_KEY_ALIAS`, `MACBOT_ANDROID_KEY_PASSWORD` before `./gradlew :androidApp:assembleRelease`. Without these variables, the output is `androidApp-release-unsigned.apk`.

This Mac mini keeps its development distribution key outside the repository:

```sh
source "$HOME/.local/share/macbot/android-signing/release.env"
./gradlew :androidApp:assembleRelease
"$ANDROID_HOME/build-tools/36.1.0/apksigner" verify androidApp/build/outputs/apk/release/androidApp-release.apk
```

Keep this key for later updates. Debug and release use different signatures; uninstall the debug app before first release installation, then use `adb install -r` for subsequent release updates. Signing keys and passwords are never committed.

Android notifications use channels `needs-you`, `completed`, `messages`. Grant notification permission when requested. The foreground service owns the long-lived main connection; screen streams open only while the Computer page is visible. Notification actions use the same idempotent protocol writes as in-app actions.

## Verification

Phase evidence and emulator screenshots are stored under `verification/`. The integrator can archive them in `docs/progress/`. See `verification/STATUS.md` for completed checks and remaining integration dependencies.

For repeatable performance checks on this M4 Mac, use `-gpu host`. The AVD default has GPU acceleration disabled and uses SwiftShader; keep software-rendered measurements separate. Verified hardware backend: Android Emulator OpenGL ES Translator (Apple M4).

```sh
adb shell am force-stop bot.mac.mobile
adb shell am start -W -n bot.mac.mobile/.MainActivity
adb shell dumpsys gfxinfo bot.mac.mobile reset
# Navigate/scroll the scenario, then:
adb shell dumpsys gfxinfo bot.mac.mobile
adb shell dumpsys meminfo bot.mac.mobile
```

Measured release samples and their limitations are in `verification/S5-performance.json`.

Co-Authored-By: Codex <noreply@openai.com>
