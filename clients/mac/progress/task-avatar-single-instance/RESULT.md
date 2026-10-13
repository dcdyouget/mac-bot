# Task avatar / work-process GUI verification

Date: 2026-10-13

## Build and single-instance control

- Repository HEAD: `a58811c` on `main`.
- Stopped the only pre-existing desktop client process found: PID `34032`, executable from `/Users/gongshaojie/Project/mac-bot-desktop-s8/.../Mac Bot.app`.
- Left the server running. It listened on port `7788` throughout; the observed PID changed from `43111` before the UI check to `89757` at the end.
- Ran from `clients/mac/app`: `npm run tauri -- build --debug --bundles app` — succeeded. Vite printed its existing >500 kB chunk advisory.
- Launched only `clients/mac/app/src-tauri/target/debug/bundle/macos/Mac Bot.app`, PID `88033`.
- App executable SHA-256: `a524f6c60af2370ce7af3b78c9e4f41df08ef2899c296696eac2373aa6a4e6d8`.
- Closed the verification app after inspection; no `macbot-desktop` process remained.

## Observations

- Connected through the saved Host entry and opened the existing 主会话.
- The accessibility tree placed the long Bot response immediately before its `任务 · 确认创建 Bot「调研员」` task card.
- Opening the task's `工作过程` and using its back arrow returned to the same task row in the conversation; it did not jump to the conversation top.
- In the returned chat screenshot, one avatar is aligned at the left edge of the task/status row, followed by the completion mark, task title, duration, and details action. No second avatar is visible in that row.
- Limitation: the long response body was present in the accessibility tree but was not painted in the chat viewport during this run; scrolling away from the task row exposed a blank visual area. Therefore this run confirms the single avatar on the visible task row and the preserved return anchor, but cannot visually prove whether the long response body has an additional avatar at its bottom.
- No message was sent. No Question, approve, reject, or pending-action control was touched. The server was not stopped.

## Screenshots

- `chat-task-row.png` — task/status row with its single visible avatar.
- `work-process.png` — selected task's work-process view.
- `chat-return-anchor.png` — chat after returning to the same task row.

SHA-256:

- `chat-task-row.png`: `16ff680102dc8be9d061f3ba49750d38ca0c53946819f901a29d104854550542`
- `work-process.png`: `daca617988926bfa796a3371c64e01aea078c66133e1f53f67a63e837b34467c`
- `chat-return-anchor.png`: `1dd051ee669b50bc42f8abcdd41d659305164534e2aebb6294d29e62bc89d2fb`
