# Android task card long press: independent verification

Date: 2026-10-12

## Environment

- AVD: isolated `macbot_longpress_verify_api36`, API 36, `emulator-5582`, `-gpu host`.
- Mock: `macbotd --mock --port 17952 --password dev --home /tmp/macbot-longpress-verifier-20261012`.
- App Host: `10.0.2.2:17952` (password `dev`), chat `登录功能` (`chat_login`).
- APK: `clients/mobile/androidApp/build/outputs/apk/debug/androidApp-debug.apk`.
- APK SHA-256: `826ea78a3832ffe83c3f67a8f5b28bef5bde76aea9d1f1ea87dbd665a91af0df`.
- Source revision at build: `e087faa`; `ChatScreen.kt` also had a local uncommitted diff when this verification ran.

## Results

- `./gradlew :shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest :androidApp:assembleDebug` — **passed** (84 tasks; includes all required test gates and `assembleDebug`).
- Light mode standalone `任务 · 执行中示例`: long press opened the action sheet with reactions, `回复`, `引用`, `查看工作详情`, `引用到公告`, and `复制`. `查看工作详情` opened the matching `执行中示例` trace page. The card's `详情` short tap opened that same trace page.
- Dark mode: repeated long press opened the same action sheet; `查看工作详情` opened the matching trace page.
- A received plain text message in the Main Bot chat also opened the existing message action sheet on long press.
- Scrolling through older and newer chat history worked before and after opening the task detail.

## Scenario note

The fresh mock seeded the same standalone card title and assignment type, but its displayed message time was `2026/10/12 15:08:57`, while the writer's earlier A/B note records `14:17:24`. This verifies the same UI path against the isolated mock, with a different generated mock timestamp.

## Screenshots

- `light-task-card-before.png`, `light-task-card-menu.png`, `light-menu-open-detail.png`, `light-task-card-short-detail.png`
- `dark-task-card-before.png`, `dark-task-card-menu.png`, `dark-menu-open-detail.png`
- `plain-message-menu.png`
- `login-chat-older4.png` records the scrolled history with the standalone cards in view.
