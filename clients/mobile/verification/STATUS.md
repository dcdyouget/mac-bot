# Android verification

2026-10-09 · Android 16 / API 36 · `macbot_api36` (`emulator-5554`).

This records Android implementation, mock checks and the explicitly listed real-service checks. PLAN §6 joint acceptance is separate; only S0 full joint acceptance is currently confirmed by the integrator.

| Stage | Android scope | Emulator evidence |
| --- | --- | --- |
| S0 | Android-only KMP, main/screen connections, auth/resume/bootstrap, durable cursors, reconnect/heartbeats, StateFlow, Keystore, theme/navigation/resources, Host management, foreground service/channels | `S0-connect-initial.png`, `S0-mock-sessions.png` |
| S1 | Private chat, 17 message blocks/Unknown, attachments/delivery/drafts, trace/live/history/steer, providers/models/settings | `S1-chat-mock.png`, `S1-trace-mock.png`, `S1-settings-mock.png` |
| S2 | Main Bot, groups/announcement/artifacts/members/review, questions/approvals, workbench, Bot CRUD/templates | `S2-workbench-mock.png`, `S2-group-mock.png`, `S2-group-announcement-mock.png`, `S2-bots-mock.png`, `S2-approvals-questions-mock.png` |
| S3 | Canvas dashboard/trends/heatmaps/CSV, skill CRUD/import/drafts/publish/per-Bot enablement, search/filter/navigation | `S3-dashboard-mock.png`, `S3-dashboard-trend-mock.png`, `S3-skills-create-mock.png`, `S3-skills-detail-mock.png`, `S3-search-mock.png` |
| S4 | Screen binary frames/render-before-ack, quality/tabs/zoom, touch/keyboard takeover and release on exit, routine pause/resume/history, notifications/actions | `S4-computer-view-mock.png`, `S4-computer-takeover-mock.png`, `S4-computer-release-mock.png`, `S4-routines-mock.png` |
| S5 | Signed release APK, R8/resource shrink, fresh install/connect, cold launch/memory/frame measurements | `S5-release-fresh-install.png`, `S5-release-connected.png`, `S5-performance.json`, `S5-dark-theme.png` |

## Validation

- `:shared:commonTest :shared:androidUnitTest`: 54 tests passed in 17 suites. Includes all 168 fixture values, request matching, resume/replay ordering, reconnect idempotency, multi-Host state isolation, state/trace merge, frame parsing/render-before-ack, bounded input ordering, real OkHttp heartbeat transport, concurrent identical writes, failed/cancelled retry IDs and full skill-detail refresh after save/publish.
- Direct `am instrument` on a distribution-signed Debug build: 4 tests passed (Keystore round-trip and 3 notification-ledger regressions). The optimized signed Release was restored with `adb install -r`. The release instrumentation attempt discovered zero tests because R8 removed runner dependencies; it is not counted as a pass. Use the data-preserving workflow in README; Gradle connected tests can uninstall the target app.
- `python3 protocol/kotlin/generate.py --check`: 220 schema models; 168 fixture values across 94 files. Unknown fields/types round-trip losslessly. Generator CLI regression suite: 5 tests passed, including read-only stale/missing-output checks and malformed-fixture failure before writes.
- Debug/signed Release and lint pass. The latest source also exposes density-aware left-swipe actions for pin/mute/group completion. `apksigner verify` validates the external development distribution key; no APK/key/password is committed.
- The earlier S5 signed release was uninstalled/reinstalled from an empty app profile, then Host `10.0.2.2:7789` / `dev` was added again: sessions, direct-chat user echo/delivery, task replay, group announcement and artifacts, Bot create/list/delete and group creation verified. Mock chat echo does not represent a real model response.
- Computer verified on the deployed takeover mock: driver becomes `user`, two tabs are available, touch/keyboard send, exit returns driver to `bot`. The final compact landscape layout preserves a usable viewport.
- Routine pause/resume verified using an explicitly named Android test routine created through RPC; mobile routine editing is intentionally unavailable per DESIGN.
- The foreground remote-messaging service runs, and all three channels exist. Notification routing handles origin chats, project reviews, takeover message blocks, Host selection and replay deduplication; true scheduled push delivery remains a joint real-service check. Android also executed `allow_once` on the deterministic mock approval and answered its `safe` question option; both disappeared from the UI and RPC bootstrap pending lists.

## Limits and pending joint checks

- Real provider response and existing-history display passed on Android against deployed server `ae3009b`: the app sent `Please only reply android-heartbeat-608c653`, received `android-heartbeat-608c653`, and RPC history confirms both known `text.markdown` blocks with `streaming=false`. Evidence: `S1-production-heartbeat-chat.png` and `S1-production-message-evidence.json`. This is partial S1 evidence, not full streaming/file/bash/recovery joint acceptance.
- The new private-chat header entry opened real chat-scoped replay: start, model request, the matching response and completed run are readable; RPC confirms 4 items (`aseq` 1–4, `live=false`). Evidence: `S1-production-chat-trace.png`, `S1-production-trace-evidence.json`.
- Real Android dashboard summary matches a near-contemporaneous 30-day RPC sample: 231,196 input+output tokens, 66 model requests, unpriced cost. Evidence: `S3-production-dashboard.png`, `S3-production-dashboard-evidence.json`; simultaneous two-client parity remains a joint check.
- Real streaming, file/bash execution, kill/restart recovery, two concurrent groups and live intervention, real Chrome login takeover and scheduled notifications remain joint checks with server-mac and integrator. Do not treat mock screenshots as their acceptance.
- Mock search previously returned a fixed empty array; server `0e882ac` supplies scenario results and filters. RPC and Android UI returned a PRD artifact and three matching messages; message filtering and navigation into the login group passed after deployment.
- Mock member/pending-reference repair `f368824` is deployed: login project/announcement now contain four members. Android shows named roles/status/current-task actions in a horizontal status strip; group and announcement screenshots were refreshed.
- Software Vulkan/SwiftShader and hardware Apple M4 performance samples are separate. Hardware-emulator samples are not physical-device claims; one emulator process exited 139 after QEMU thread hangs and was restarted with user data preserved. No app crash is attributed without an app crash log.
- `S1-chat-initial.png` is an early diagnostic screenshot, not phase completion evidence. Older skill-create evidence predates the latest mock reset.

## Continuation fixes

- MainConnection sends the protocol `ping` request every 20 seconds. Ktor's OkHttp session rejects an explicit `Frame.Ping` write; the previous implementation therefore disconnected at each heartbeat. A real OkHttp/MockWebServer regression crosses multiple heartbeat intervals and confirms a later request still succeeds.
- Identical simultaneous writes now have independent request IDs; failed/cancelled calls release their lease and reuse their own retry ID. Skills save/publish refresh the full detail, not only the list.
- The foreground service subscribes before connection initialization, seeds notification history at hello before replay, skips streaming placeholders, and notifies each final message once. Host connection failures are visible in localized UI text.
- Main/direct private chats expose their chat-scoped trace from the header even when messages have no assignment ID.
