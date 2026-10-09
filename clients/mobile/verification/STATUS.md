# Android verification

2026-10-09: Debug APK built and installed on `macbot_api36` / `emulator-5554` (Android 16, API 36).

- `:shared:commonTest :shared:androidUnitTest`: 46 tests passed, including all 165 fixture values, bootstrap/replay ordering, request ID matching, reconnect idempotency, multi-Host isolation, screen render-before-ack, state/trace merge and Android persistence.
- `:androidApp:connectedDebugAndroidTest`: Keystore round-trip passed (1 test).
- Emulator connected to the server-mac test mock at `10.0.2.2:7789` using `dev`; sessions are visible. Evidence: `S0-mock-sessions.png`.
- Foreground remote messaging service is running; notification permission granted.
- Main mock gateway is deployed; `chat_main.kind=main` now renders the main Bot at the top.
- client-android S0 complete: Android main mock connection, sessions, persistence, core tests and screenshot verified.
- S1–S5 interfaces are implemented; feature scenarios and signed release/performance checks remain in progress. No later stage is declared complete.

- Feature UI checks: direct-chat send/echo and delivered message (`S1-chat-mock.png`); task trace replay (`S1-trace-mock.png`); named Bot/project workbench groups (`S2-workbench-mock.png`); skill creation (`S3-skills-create-mock.png`).
- Added input backpressure tests: important start/end/key events keep order; transient motion may drop under pressure.
- Latest Debug build, 46 host tests and lint passed. Computer takeover layout/exit release and current signed Release verification continue.
