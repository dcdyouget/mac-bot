# Computer lifecycle retest — 2026-10-12

## Result

**Build gates passed; GUI acceptance is blocked by the independent Android emulator's network path.** The API 36 AVD booted, but its first connection attempt to the host mock failed with `Network is unreachable`. Per the verification instruction, the AVD was not repeatedly restarted or probed. No APK was installed and no application UI or mock RPC was exercised in this retest.

## Build and artifact

- Command, from `clients/mobile/`: `ANDROID_HOME=/Users/gongshaojie/Library/Android/sdk ./gradlew :shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest :androidApp:assembleDebug`
- Result: `BUILD SUCCESSFUL in 28s`; 84 actionable tasks (12 executed, 72 up-to-date).
- APK SHA-256 (assembled from the main worktree): `9c9a1c774a49a84bd153372ef0a2e348c83c69e49e5865c5d83eb3bcec276eff`
- Gradle emitted daemon handshake warnings and existing Kotlin deprecation/redundant-null warnings; they did not fail the gates.

## GUI environment

- Independent AVD: `macbot_takeover_verify_api36_20261012`, serial `emulator-5576`, Android API 36.
- Boot completed (`sys.boot_completed=1`).
- Host A mock: `10.0.2.2:18573`, home `/tmp/macbot-takeover-retest-20261012/host-a`.
- Host B mock: `10.0.2.2:18574`, home `/tmp/macbot-takeover-retest-20261012/host-b`.
- First and only network probe: `adb -s emulator-5576 shell toybox nc -z -w 2 10.0.2.2 18573`
- Output: `nc: connect: Network is unreachable`.
- Passive connectivity diagnostics showed Wi-Fi address `10.0.2.16`, route `10.0.2.0/24` with default gateway `10.0.2.2`, and `PARTIAL_CONNECTIVITY`.
- Screenshot: [retest-network-failure.png](retest-network-failure.png) (home screen, captured after the failed probe).
- AVD and both mock processes were stopped after recording the failure; `emulator-5576` is absent from `adb devices`.

## Acceptance coverage

Not run because the mock was unreachable: takeover, three or more light/dark theme changes, observer frame progression and `driver=user`, system Back release to `driver=bot`, deep link/Host switching and old Host release, skill GUI/restart-history.

For the isolated-AVD attempt only, no observer state or RPC calls exist. Do not treat screenshots or evidence from the earlier failed implementation as evidence for this build. The successful original-AVD evidence follows below. No product or shared files were changed, and no commit was created.

## Original AVD GUI retest — 2026-10-12

The network-diagnose agent confirmed that the untouched original API 36 AVD (`macbot_api36`, `emulator-5554`) could reach its mock at `10.0.2.2:7789`. I installed the APK above with `adb install -r`; I did not wipe the AVD. The mock was agent-owned PID 73531, password `dev`, `MACBOT_HOME=/tmp/macbot-avd-network-diagnose`. Admin log polling returned no entries, so the RPC evidence below is the server-provided screen state and the Android client's reloaded protocol snapshot, rather than a raw server request log.

### Computer takeover

- Added local Host `NetworkTest` at `10.0.2.2:7789`, local ID `3e8e7941-054e-4049-bb14-8842fc32560c`. For Host-switch coverage, added `SwitchHost` pointing at that same mock endpoint, local ID `4e5bcc4e-dede-468b-a210-3ebc0374e923`.
- Opened `macbot://computer/bot_product?host_id=<local-host-id>`. The `bot_product` screen showed two mock tabs and a received frame. Tapping “接管并处理” changed the server-provided `ScreenState.driver` to `user`; the UI displayed “你正在操作（Bot 已暂停）”.
- While takeover remained active, changed Android system night mode dark → light → dark. Both screenshots retain the user status and the Computer route. The mock image itself is static; screen frames continued arriving (frame age 0 ms, observed stream FPS values 20–28 after transitions).
- Android Back exited Computer. Reopening the same screen showed “Bot 操作中” (`driver=bot`) and a fresh frame (`24 fps`, age `0 ms`), confirming release on Back.
- Reopened Computer through `SwitchHost`, started takeover, then delivered a Computer deep link selecting `NetworkTest`. The selected local Host ID changed from SwitchHost to NetworkTest; the reopened screen showed “Bot 操作中” (`driver=bot`) with continuing frames (`28 fps`, age `0 ms`). Since both local Host records intentionally target the same mock, the new screen's bot state confirms release affected the shared remote session during the Host change.
- Screenshots: `retest-computer-takeover.png`, `retest-computer-light-user.png`, `retest-computer-dark-user.png`, `retest-computer-back-state-bot.png`, `retest-host-switch-takeover-old.png`, and `retest-host-switch-after.png`.

### Loaded skill and history

- In the `官网改版` group, selected enabled skill `web-auth-patterns`, mentioned `编码`, and sent `Review auth@编码`. The delivered message showed the skill chip and “已加载技能 web-auth-patterns”.
- Force-stopped and reopened the app without clearing data, reopened the group, and confirmed the history still showed the loaded-skill marker in both light and dark system themes.
- The reconnected Android snapshot for `NetworkTest` had `lastSeq=94`. In `chat_web`, message `msg_01a12f47-6ed5-75f1-865a-8da8b1e541d0` contained `loaded_skills=[{name:"web-auth-patterns", bot_id:"bot_code"}]` and a `bot_code` mention. This is the protocol state evidence for the live loaded-skill update and history restoration.
- Sent `/unknown`: the picker said “没有匹配的技能”; the stored message `msg_01a12f48-e009-7664-a1fc-52293a48491e` had `loaded_skills=[]` and remained plain `/unknown` text.
- Typed disabled skill `/prd-template`: the picker also said “没有匹配的技能”; it remained composer text with no selectable skill chip. Screenshots: `retest-skill-picker.png`, `retest-skill-draft.png`, `retest-skill-message-draft.png`, `retest-skill-message-sent-light.png`, `retest-skill-history-light.png`, `retest-skill-history-dark.png`, `retest-skill-unknown-draft.png`, `retest-skill-unknown-sent.png`, and `retest-skill-disabled-full.png`.

Final original-AVD GUI checks passed for Computer takeover, dark/light changes during takeover, Back release, Host deep-link release, frame delivery, loaded-skill update/history, and unknown/disabled skill handling. The isolated AVD attempt above remains recorded as the initial environment failure. No product/shared files changed and no commit was created.
