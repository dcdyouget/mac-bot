# Android verification

2026-10-09 · Android 16 / API 36 · `macbot_api36` (`emulator-5554`).

This records Android implementation and mock checks. PLAN §6 joint acceptance against the real service is separate; only S0 joint acceptance is currently confirmed by the integrator.

| Stage | Android scope | Emulator evidence |
| --- | --- | --- |
| S0 | Android-only KMP, main/screen connections, auth/resume/bootstrap, durable cursors, reconnect/heartbeats, StateFlow, Keystore, theme/navigation/resources, Host management, foreground service/channels | `S0-connect-initial.png`, `S0-mock-sessions.png` |
| S1 | Private chat, 17 message blocks/Unknown, attachments/delivery/drafts, trace/live/history/steer, providers/models/settings | `S1-chat-mock.png`, `S1-trace-mock.png`, `S1-settings-mock.png` |
| S2 | Main Bot, groups/announcement/artifacts/members/review, questions/approvals, workbench, Bot CRUD/templates | `S2-workbench-mock.png`, `S2-group-mock.png`, `S2-group-announcement-mock.png`, `S2-bots-mock.png`, `S2-approvals-questions-mock.png` |
| S3 | Canvas dashboard/trends/heatmaps/CSV, skill CRUD/import/drafts/publish/per-Bot enablement, search/filter/navigation | `S3-dashboard-mock.png`, `S3-dashboard-trend-mock.png`, `S3-skills-create-mock.png`, `S3-skills-detail-mock.png`, `S3-search-mock.png` |
| S4 | Screen binary frames/render-before-ack, quality/tabs/zoom, touch/keyboard takeover and release on exit, routine pause/resume/history, notifications/actions | `S4-computer-view-mock.png`, `S4-computer-takeover-mock.png`, `S4-computer-release-mock.png`, `S4-routines-mock.png` |
| S5 | Signed release APK, R8/resource shrink, fresh install/connect, cold launch/memory/frame measurements | `S5-release-fresh-install.png`, `S5-release-connected.png`, `S5-performance.json`, `S5-dark-theme.png` |

## Validation

- `:shared:commonTest :shared:androidUnitTest`: 48 tests passed, including the origin-chat steering regression. Includes all 166 fixture values, request matching, resume/replay ordering, reconnect idempotency, multi-Host state isolation, state/trace merge, frame parsing/render-before-ack and bounded input ordering.
- `:androidApp:connectedDebugAndroidTest`: Keystore round-trip passed (1 test).
- `python3 protocol/kotlin/generate.py --check`: 210 schema models; 166 fixture values across 92 files. Unknown fields/types round-trip losslessly.
- Debug/signed Release and lint pass. The latest source also exposes density-aware left-swipe actions for pin/mute/group completion. `apksigner verify` validates the external development distribution key; no APK/key/password is committed.
- Final signed release was uninstalled/reinstalled from an empty app profile, then Host `10.0.2.2:7789` / `dev` was added again: sessions, direct-chat user echo/delivery, task replay, group announcement and artifacts, Bot create/list/delete and group creation verified. Mock chat echo does not represent a real model response.
- Computer verified on the deployed takeover mock: driver becomes `user`, two tabs are available, touch/keyboard send, exit returns driver to `bot`. The final compact landscape layout preserves a usable viewport.
- Routine pause/resume verified using an explicitly named Android test routine created through RPC; mobile routine editing is intentionally unavailable per DESIGN.
- The foreground remote-messaging service runs, and all three channels exist. Notification routing handles origin chats, project reviews, takeover message blocks, Host selection and replay deduplication; true scheduled push delivery remains a joint real-service check. Android also executed `allow_once` on the deterministic mock approval and answered its `safe` question option; both disappeared from the UI and RPC bootstrap pending lists.

## Limits and pending joint checks

- Real provider streaming, file/bash execution, kill/restart recovery, two concurrent groups and live intervention, real Chrome login takeover and scheduled notifications depend on server-mac and integrator. Do not treat mock screenshots as their acceptance.
- Mock search previously returned a fixed empty array; server `0e882ac` supplies scenario results and filters. RPC and Android UI returned a PRD artifact and three matching messages; message filtering and navigation into the login group passed after deployment.
- Mock member/pending-reference repair `f368824` is deployed: login project/announcement now contain four members. Android shows named roles/status/current-task actions in a horizontal status strip; group and announcement screenshots were refreshed.
- Software Vulkan/SwiftShader and hardware Apple M4 performance samples are separate. Hardware-emulator samples are not physical-device claims; one emulator process exited 139 after QEMU thread hangs and was restarted with user data preserved. No app crash is attributed without an app crash log.
- `S1-chat-initial.png` is an early diagnostic screenshot, not phase completion evidence. Older skill-create evidence predates the latest mock reset.
