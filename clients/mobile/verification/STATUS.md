# Android verification

2026-10-09: Debug APK built and installed on `macbot_api36` / `emulator-5554` (Android 16, API 36).

- `:shared:commonTest :shared:androidUnitTest`: 44 tests passed, including all 104 fixture values, bootstrap/replay ordering, request ID matching, reconnect idempotency, multi-Host isolation, screen render-before-ack, state/trace merge and Android persistence.
- `:androidApp:connectedDebugAndroidTest`: Keystore round-trip passed (1 test).
- Emulator connected to the server-mac test mock at `10.0.2.2:7789` using `dev`; sessions are visible. Evidence: `S0-mock-sessions.png`.
- Foreground remote messaging service is running; notification permission granted.
- Mock gateway published in `54349d8`; the server-mac test snapshot has `chat_main.kind=direct`; server-mac has been notified.
- client-android S0 complete: Android main mock connection, sessions, persistence, core tests and screenshot verified. The server-mac main-chat kind issue is tracked separately.
- S1–S5 interfaces are implemented; feature scenarios and signed release/performance checks remain in progress. No later stage is declared complete.
