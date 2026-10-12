# Android theme audit

Date: 2026-10-12

## Test setup

- Temporary API 36 arm64 AVD `macbot_theme_audit_api36`, software-rendered with SwiftShader, emulator port 5556.
- Isolated current mock on port 17889; Android connected through `10.0.2.2:17889`.
- Android gates and packaging: `:shared:commonTest`, `:shared:androidUnitTest`, `:androidApp:testDebugUnitTest`, and `:androidApp:assembleDebug` passed (`BUILD SUCCESSFUL`, 84 tasks).

## Captures

- Light: message list, chat, task detail, settings, user memory, artifact preview.
- Dark: message list, chat, task detail, settings.
- Files are adjacent PNGs named by theme and screen. The dark user-memory and artifact-preview captures could not be completed: after a transient ADB reconnect, shell input/capture hung again; the device was stopped rather than repeatedly retried.
- Compared the light message-list capture against `design/prototype/mobile.html#m-list`. Content hierarchy and navigation are present; spacing and text density differ at the 1080×2400 emulator capture size. Dark reference uses the same prototype anchor with its dark theme control.
- Artifact: mock `art_mock_brief`, project `prj_mock_review`, title `项目说明`, path `docs/brief.md`. Client request: `GET /api/v1/files?root=project&root_id=prj_mock_review&path=docs%2Fbrief.md`; preview frame loads but content returns HTTP 404.

## Translucent finding

The Android implementation reports `platformBackdropBlurSupported() = false` in `shared/src/androidMain/kotlin/bot/mac/mobile/core/platform/AndroidPlatform.kt`. The palette therefore skips translucent overlays; `MainActivity.kt` also leaves system bars opaque. This build does not provide background blur or see-through. The setting can be persisted, but the visual mode is not implemented on Android.

The separate PoC's API 36 SwiftShader measurements are p50 550 ms / p90 850 ms with blur, compared with 300 ms / 450 ms without blur; its render target adds 16 MB. This is sufficient to reject that PoC as a merge candidate under the measured software-rendered configuration, but does not establish that a hardware-accelerated implementation is infeasible.

Recommended bounded next check: on the API 36 host-GPU AVD, profile only one 30 dp bottom navigation surface and one 30 dp sheet overlay, each against an opaque baseline, using the same chat screen. Capture matching screenshots and compare five warm runs of frame timing and process memory. Keep the prototype isolated; only consider merge if blur is visually apparent and its p90 latency and memory fit the existing UI budget. Avoid whole-screen replay or expanding to additional surfaces before that check passes.

## Cleanup

The temporary AVD, mock process, and `/tmp/macbot-theme-audit-home` were removed/stopped after testing. The default `macbot_api36` AVD was not touched.
