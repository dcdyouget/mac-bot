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

The fixture contract test fails if the corpus is missing; no-fixture builds are not phase completion evidence.

## Emulator deploy

```sh
"$ANDROID_HOME/emulator/emulator" -avd macbot_api36 &
"$ANDROID_HOME/platform-tools/adb" wait-for-device
"$ANDROID_HOME/platform-tools/adb" install -r androidApp/build/outputs/apk/debug/androidApp-debug.apk
"$ANDROID_HOME/platform-tools/adb" shell am start -n bot.mac.mobile/.MainActivity
"$ANDROID_HOME/platform-tools/adb" exec-out screencap -p > verification/S0.png
```

Add a Host using `10.0.2.2:7789`, password `dev`, for mock; real host uses `10.0.2.2:7788`. Physical devices can use any reachable IP/domain, including `192.168.31.162:7788`. Each Host accepts multiple ordered addresses, `ws`/`wss`/`http`/`https`, proxy prefixes and IPv6. Passwords are encrypted with Android Keystore; Host snapshots, drafts and replay cursors persist separately per Host.

Debug APK: `androidApp/build/outputs/apk/debug/androidApp-debug.apk`. Release APK: `androidApp/build/outputs/apk/release/androidApp-release-unsigned.apk`; signing uses user-provided credentials (never committed). Run `./gradlew :androidApp:assembleRelease` for an optimized unsigned release.

Android notifications use channels `needs-you`, `completed`, `messages`. Grant notification permission when requested. The foreground service owns the long-lived main connection; screen streams open only while the Computer page is visible. Notification actions use the same idempotent protocol writes as in-app actions.

## Verification

Phase evidence and emulator screenshots are stored under `verification/`. The integrator can archive them in `docs/progress/`. See `verification/STATUS.md` for completed checks and remaining integration dependencies.

Co-Authored-By: Codex <noreply@openai.com>
