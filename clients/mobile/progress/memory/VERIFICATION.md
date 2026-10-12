# Android memory batch verification

## Build and tests

Verified source commit: `711428773d63ef928b13486964ab757b08f9b555`. Ran from `clients/mobile` on JDK 21 with the existing compile SDK 37.0 / target SDK 36 configuration:

```sh
./gradlew :shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest :androidApp:assembleDebug
```

Result: **BUILD SUCCESSFUL**. Shared host tests reported 35 suites / 140 tests; Android app unit tests reported 6 suites / 37 tests. Failures, errors, and skipped tests: 0. AGP 9.4.1 maps `:shared:commonTest` and `:shared:androidUnitTest` to the same `testAndroidHostTest` task, so shared tests are counted once. The debug APK was built at `androidApp/build/outputs/apk/debug/androidApp-debug.apk`; its Android Debug signing certificate matches the existing installed app certificate.

The complete Gradle output is in [gradle-tests.txt](gradle-tests.txt). Gradle printed four local worker connection handshake timeouts, but all requested test tasks and APK assembly completed successfully.

## Review and mock preparation

Independent review covered profile save/version handling, source rows, quota state, the v1.1 approval and announcement changes, and the integrated chat changes. The profile save path now checks that the current draft still matches the returned server snapshot before showing the saved checkmark; a generation guard prevents an older checkmark timer from hiding a newer save.

An isolated mock was prepared at `10.0.2.2:7789` with its data home under `/tmp/macbot-android-memory-verify-20261012`. Its real `memory.profile.get` response reports `onboarded=false`. [scenarios.py](scenarios.py) is a verification-only WebSocket relay for the fifth screenshot: it forwards all RPCs/events and changes only `profile.onboarded` to `true` in the response to `memory.profile.get`. A direct WebSocket smoke check returned `true` through the relay while a direct mock RPC still returned `false`. This exercises client rendering of the initialized-profile response; it does **not** validate conversational onboarding. The mock and relay were stopped after the smoke check.

## Device screenshots

Per the current unlock instruction, no candidate APK was installed and no app UI or screenshot was captured. The existing app data and Host records remain untouched. All five screenshots are pending an explicit Mac mini unlock notice:

- `01-default.png` — pending
- `02-profile-edit.png` — pending
- `03-conflict.png` — pending
- `04-quota-full.png` — pending
- `05-onboarded.png` — pending; use the documented relay fixture and state that conversational onboarding was not exercised
